//! Rig patterns: the named parts a character is made of.
//!
//! # Why a pattern is data and not a type
//!
//! `buzz-act` already knows one skeleton by heart — `figure::Joint`, eleven
//! bones of a person, written as a Rust enum. That is exactly the right shape
//! for the code that animates it, because a walk cycle is `ThighL` and `KneeL`
//! by name and the compiler should say so. It is exactly the wrong shape for
//! the animator, who has a horse, a bird, or an anglepoise lamp, and who wants
//! the same rig-by-dropping-parts that a person gets.
//!
//! So the *slots* live here, as data. A [`RigPattern`] is a list of places a
//! drawing can go — "Elbow L", hanging off "Shoulder L", pointing roughly
//! down, about this long. Assemble artwork into one and what comes out is an
//! ordinary [`crate::Armature`]: bones and angles, with nothing left in it
//! that remembers where it came from.
//!
//! # The biped pattern is a contract
//!
//! [`RigPattern::biped`]'s slots are in the same order, with the same names and
//! the same parents, as `buzz_act::figure::Joint`. A performance addresses
//! bones by *index* — `Joint::ThighL.index()` is 7 and always will be — so
//! shuffling this table would silently animate the wrong limb rather than fail
//! to compile. `buzz-act` has a test that holds the two side by side.
//!
//! # Angles
//!
//! As everywhere in this crate: radians, relative to the parent, y downwards,
//! so a positive angle turns clockwise on screen. [`Slot::rest_len`] is a
//! *fraction of the figure's height*, so one table describes a child, an adult
//! and a giant.

use crate::{Direction, JointLimits, RotationMode, wrap_pi};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which side of the body a slot belongs to.
///
/// Auto-assignment leans on this hard: a part whose name says "left" must not
/// land on the right arm, because a character whose arms are swapped is
/// discovered three shots later, while an empty slot is discovered immediately
/// in the panel that is already open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// A spine, a head, a tail: there is only one of it.
    Either,
    Left,
    Right,
}

/// How a character's parts are held together.
///
/// `Bones` is an armature (IK, the director, lip-sync); `Layers` is Animate's
/// layer parenting — one layer per part, each turning about its joint and
/// following the layer of the part it hangs off, posed with Free Transform.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RigKind {
    #[default]
    Bones,
    Layers,
}

impl RigKind {
    /// `"bones"` / `"layers"`, as the Cast shows it and the CLI takes it.
    pub fn label(self) -> &'static str {
        match self {
            RigKind::Bones => "bones",
            RigKind::Layers => "layers",
        }
    }

    /// The inverse of [`RigKind::label`], case-insensitively.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "bones" | "bone" => Some(RigKind::Bones),
            "layers" | "layer" => Some(RigKind::Layers),
            _ => None,
        }
    }

    /// The one line the kind picker says about layer rigs.
    pub const LAYERS_HINT: &'static str = "Layer rig: pose with Free Transform; the director, gestures, blinks and \
         lip sync work on it; joint limits are bone-rig only";
}

/// One named place in a rig, and the bone that will stand in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    /// What the bone is called once it exists, and what the panel shows.
    pub name: String,
    /// The slot this one hangs off. Always *earlier* in the list, so a pattern
    /// can be walked in one forward pass and cannot describe a cycle.
    pub parent: Option<usize>,
    /// Which way the bone points when nothing better is known, relative to its
    /// parent.
    pub rest_angle: f64,
    /// How long the bone is, as a fraction of the figure's height.
    ///
    /// Used for slots nobody dropped a drawing on, so the skeleton stays
    /// complete: a character with no separate chest drawing still has a chest
    /// to turn. A slot that was filled takes its length from the drawing.
    pub rest_len: f64,
    /// How far the joint may turn, **relative to the bone's rest angle** (the
    /// rest angle is only known once the bone is fitted to its drawing, so the
    /// auto-rigger turns these into the bone's absolute [`JointLimits`] with
    /// [`JointLimits::from_relative`]). `None` is a free joint.
    pub limits: Option<JointLimits>,
    pub side: Side,
    /// A slot the character does not really work without. The panel says which
    /// of these are still empty, rather than leaving the animator to find out
    /// when a walk cycle comes out on one leg.
    pub required: bool,
    /// Other things people call this part, for matching layer names against.
    ///
    /// Order does not matter — the *longest* alias that matches wins, so
    /// "forearm" beats "arm" without either having to be listed first.
    pub aliases: Vec<String>,
}

/// A whole skeleton's worth of slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RigPattern {
    pub name: String,
    pub slots: Vec<Slot>,
}

/// Terse constructor, so the tables below read as tables.
fn slot(
    name: &str,
    parent: Option<usize>,
    rest_angle: f64,
    rest_len: f64,
    side: Side,
    required: bool,
    aliases: &[&str],
) -> Slot {
    Slot {
        name: name.to_string(),
        parent,
        rest_angle,
        rest_len,
        limits: None,
        side,
        required,
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
    }
}

use std::f64::consts::{FRAC_PI_2, PI};

/// Straight up the screen, which is where a spine points.
const UP: f64 = -FRAC_PI_2;
/// Straight down, which is where a leg points.
const DOWN: f64 = FRAC_PI_2;

impl RigPattern {
    /// **A person.** Eleven bones, in `buzz_act::figure::Joint` order.
    ///
    /// The proportions are the eight-heads drawing convention and the same
    /// numbers `figure::build` uses — see the notes there for why each landmark
    /// sits where it does.
    pub fn biped() -> Self {
        let mut pattern = Self {
            name: "Biped".into(),
            slots: vec![
                // The spine, upwards from the pelvis.
                slot(
                    "Hips",
                    None,
                    UP,
                    0.23,
                    Side::Either,
                    true,
                    &["hips", "hip", "pelvis", "waist", "root", "lower body"],
                ),
                slot(
                    "Chest",
                    Some(0),
                    0.0,
                    0.12,
                    Side::Either,
                    true,
                    &["chest", "torso", "ribs", "body", "spine", "upper body"],
                ),
                slot(
                    "Head",
                    Some(1),
                    0.0,
                    0.18,
                    Side::Either,
                    true,
                    &["head", "skull", "face", "neck"],
                ),
                // Arms. A hair off vertical so a rest pose has arms beside the
                // body rather than fused to it — see `figure::build`.
                slot(
                    "Shoulder L",
                    Some(1),
                    PI + 0.26,
                    0.155,
                    Side::Left,
                    true,
                    &["shoulder", "upper arm", "bicep", "arm"],
                ),
                slot(
                    "Elbow L",
                    Some(3),
                    0.06,
                    0.145,
                    Side::Left,
                    true,
                    &["elbow", "forearm", "lower arm", "hand"],
                ),
                slot(
                    "Shoulder R",
                    Some(1),
                    PI - 0.26,
                    0.155,
                    Side::Right,
                    true,
                    &["shoulder", "upper arm", "bicep", "arm"],
                ),
                slot(
                    "Elbow R",
                    Some(5),
                    0.06,
                    0.145,
                    Side::Right,
                    true,
                    &["elbow", "forearm", "lower arm", "hand"],
                ),
                // Legs, roots of their own: a bone cannot have children at both
                // ends, and the hips bone's tip is up at the ribs.
                slot(
                    "Thigh L",
                    None,
                    DOWN + 0.09,
                    0.235,
                    Side::Left,
                    true,
                    &["thigh", "upper leg", "leg", "hip"],
                ),
                slot(
                    "Knee L",
                    Some(7),
                    -0.04,
                    0.235,
                    Side::Left,
                    true,
                    &["knee", "shin", "lower leg", "calf", "foot", "boot"],
                ),
                slot(
                    "Thigh R",
                    None,
                    DOWN - 0.09,
                    0.235,
                    Side::Right,
                    true,
                    &["thigh", "upper leg", "leg", "hip"],
                ),
                slot(
                    "Knee R",
                    Some(9),
                    -0.04,
                    0.235,
                    Side::Right,
                    true,
                    &["knee", "shin", "lower leg", "calf", "foot", "boot"],
                ),
            ],
        };
        pattern.apply_limits(&JointLimitDefaults::anatomical());
        pattern
    }

    /// **A person with wrists, ankles and eyebrows of their own.** Seventeen
    /// bones: [`RigPattern::biped`]'s eleven, unchanged and in the same order,
    /// then `Wrist L/R` (off the elbows), `Ankle L/R` (off the knees) and
    /// `Brow L/R` (off the head).
    ///
    /// The new slots are *appended* and never renumbered, so every performance,
    /// pose and lip-sync that addresses bones 0–10 by index keeps working, and a
    /// pose written for eleven bones applies to this rig "as far as it goes".
    /// Image-built characters use this pattern (`buzz_ai::character`); the
    /// built-in puppet and the director keep [`RigPattern::biped`].
    ///
    /// "hand", "foot" and "boot" are aliases of the wrist and ankle here, so
    /// they are taken off the elbow and knee (where `biped` still has them): the
    /// longest alias wins and an equal one would otherwise go to the earlier slot.
    pub fn biped_full() -> Self {
        let mut slots = Self::biped().slots;
        for slot in slots.iter_mut() {
            match slot.name.as_str() {
                "Elbow L" | "Elbow R" => slot.aliases.retain(|a| a != "hand"),
                "Knee L" | "Knee R" => slot.aliases.retain(|a| a != "foot" && a != "boot"),
                _ => {}
            }
        }
        // Hands point on from the forearm, so no rest turn of their own.
        slots.push(slot(
            "Wrist L",
            Some(4),
            0.0,
            0.06,
            Side::Left,
            false,
            &["wrist", "hand", "palm", "fingers"],
        ));
        slots.push(slot(
            "Wrist R",
            Some(6),
            0.0,
            0.06,
            Side::Right,
            false,
            &["wrist", "hand", "palm", "fingers"],
        ));
        // A foot points forward, a quarter turn from the shin (which points
        // down), a little short of it.
        slots.push(slot(
            "Ankle L",
            Some(8),
            -FRAC_PI_2 + 0.2,
            0.07,
            Side::Left,
            false,
            &["ankle", "foot", "toes", "boot"],
        ));
        slots.push(slot(
            "Ankle R",
            Some(10),
            -FRAC_PI_2 + 0.2,
            0.07,
            Side::Right,
            false,
            &["ankle", "foot", "toes", "boot"],
        ));
        slots.push(slot(
            "Brow L",
            Some(2),
            0.0,
            0.04,
            Side::Left,
            false,
            &["brow", "eyebrow"],
        ));
        slots.push(slot(
            "Brow R",
            Some(2),
            0.0,
            0.04,
            Side::Right,
            false,
            &["brow", "eyebrow"],
        ));
        let mut pattern = Self {
            name: "Biped Full".into(),
            slots,
        };
        pattern.apply_limits(&JointLimitDefaults::anatomical());
        pattern
    }

    /// **A four-legged animal.** A horse, a dog, a cat.
    ///
    /// The spine runs *along* the animal rather than up it, so the root bone
    /// points backwards from the shoulders and the tail carries on from it.
    pub fn quadruped() -> Self {
        Self {
            name: "Quadruped".into(),
            slots: vec![
                slot(
                    "Body",
                    None,
                    0.0,
                    0.42,
                    Side::Either,
                    true,
                    &["body", "torso", "barrel", "chest", "spine", "back"],
                ),
                slot(
                    "Hindquarters",
                    Some(0),
                    0.0,
                    0.20,
                    Side::Either,
                    false,
                    &["hindquarters", "rump", "hips", "pelvis", "haunch"],
                ),
                slot("Tail", Some(1), 0.35, 0.30, Side::Either, false, &["tail"]),
                slot("Neck", None, -0.7, 0.24, Side::Either, true, &["neck"]),
                slot(
                    "Head",
                    Some(3),
                    0.5,
                    0.18,
                    Side::Either,
                    true,
                    &["head", "skull", "face", "muzzle"],
                ),
                slot(
                    "Foreleg L",
                    None,
                    DOWN,
                    0.24,
                    Side::Left,
                    true,
                    &["foreleg", "front leg", "shoulder", "upper foreleg"],
                ),
                slot(
                    "Fore Cannon L",
                    Some(5),
                    0.0,
                    0.24,
                    Side::Left,
                    true,
                    &["cannon", "front hoof", "front foot", "knee", "fore cannon"],
                ),
                slot(
                    "Foreleg R",
                    None,
                    DOWN,
                    0.24,
                    Side::Right,
                    true,
                    &["foreleg", "front leg", "shoulder", "upper foreleg"],
                ),
                slot(
                    "Fore Cannon R",
                    Some(7),
                    0.0,
                    0.24,
                    Side::Right,
                    true,
                    &["cannon", "front hoof", "front foot", "knee", "fore cannon"],
                ),
                slot(
                    "Hindleg L",
                    None,
                    DOWN,
                    0.24,
                    Side::Left,
                    true,
                    &["hindleg", "back leg", "rear leg", "thigh", "haunch"],
                ),
                slot(
                    "Hind Cannon L",
                    Some(9),
                    0.0,
                    0.24,
                    Side::Left,
                    true,
                    &["hock", "back hoof", "back foot", "hind cannon", "rear hoof"],
                ),
                slot(
                    "Hindleg R",
                    None,
                    DOWN,
                    0.24,
                    Side::Right,
                    true,
                    &["hindleg", "back leg", "rear leg", "thigh", "haunch"],
                ),
                slot(
                    "Hind Cannon R",
                    Some(11),
                    0.0,
                    0.24,
                    Side::Right,
                    true,
                    &["hock", "back hoof", "back foot", "hind cannon", "rear hoof"],
                ),
            ],
        }
    }

    /// **Something with wings.** A bird, a bat, a dragon.
    pub fn bird() -> Self {
        Self {
            name: "Bird".into(),
            slots: vec![
                slot(
                    "Body",
                    None,
                    0.0,
                    0.45,
                    Side::Either,
                    true,
                    &["body", "torso", "breast", "chest", "spine"],
                ),
                slot(
                    "Tail",
                    Some(0),
                    0.0,
                    0.30,
                    Side::Either,
                    false,
                    &["tail", "tail feathers"],
                ),
                slot("Neck", None, -0.6, 0.16, Side::Either, false, &["neck"]),
                slot(
                    "Head",
                    Some(2),
                    0.4,
                    0.16,
                    Side::Either,
                    true,
                    &["head", "skull", "beak", "face"],
                ),
                slot(
                    "Wing L",
                    None,
                    PI - 0.4,
                    0.34,
                    Side::Left,
                    true,
                    &["wing", "upper wing", "shoulder"],
                ),
                slot(
                    "Primaries L",
                    Some(4),
                    0.2,
                    0.34,
                    Side::Left,
                    false,
                    &[
                        "primaries",
                        "wing tip",
                        "outer wing",
                        "lower wing",
                        "feathers",
                    ],
                ),
                slot(
                    "Wing R",
                    None,
                    PI + 0.4,
                    0.34,
                    Side::Right,
                    true,
                    &["wing", "upper wing", "shoulder"],
                ),
                slot(
                    "Primaries R",
                    Some(6),
                    0.2,
                    0.34,
                    Side::Right,
                    false,
                    &[
                        "primaries",
                        "wing tip",
                        "outer wing",
                        "lower wing",
                        "feathers",
                    ],
                ),
                slot(
                    "Leg L",
                    None,
                    DOWN,
                    0.18,
                    Side::Left,
                    false,
                    &["leg", "foot", "claw", "talon"],
                ),
                slot(
                    "Leg R",
                    None,
                    DOWN,
                    0.18,
                    Side::Right,
                    false,
                    &["leg", "foot", "claw", "talon"],
                ),
            ],
        }
    }

    /// **A hinged prop.** An anglepoise lamp, a crane, a level-crossing gate.
    ///
    /// Three bones and no sides. It is here because half of what wants rigging
    /// in a film is not a character, and reaching for the biped to animate a
    /// desk lamp means eight empty slots and a panel full of warnings.
    pub fn prop() -> Self {
        Self {
            name: "Prop".into(),
            slots: vec![
                slot(
                    "Base",
                    None,
                    UP,
                    0.30,
                    Side::Either,
                    true,
                    &["base", "foot", "stand", "root", "mount", "body"],
                ),
                slot(
                    "Arm",
                    Some(0),
                    0.0,
                    0.40,
                    Side::Either,
                    true,
                    &["arm", "boom", "jib", "shaft", "stem", "middle"],
                ),
                slot(
                    "Tip",
                    Some(1),
                    0.0,
                    0.30,
                    Side::Either,
                    false,
                    &["tip", "head", "shade", "hook", "end", "lamp"],
                ),
            ],
        }
    }

    /// Every pattern that ships with the tool.
    pub fn builtin() -> Vec<RigPattern> {
        vec![
            Self::biped(),
            Self::quadruped(),
            Self::bird(),
            Self::prop(),
            Self::biped_full(),
        ]
    }

    /// A built-in pattern by name, as stored on a rig that was built from one.
    pub fn named(name: &str) -> Option<RigPattern> {
        Self::builtin().into_iter().find(|p| p.name == name)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn slot_named(&self, name: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.name == name)
    }

    /// [`Self::slot_named`], also taking a slot's display name
    /// ([`canonical_slot`]): "Upper Arm R" finds "Shoulder R". For reading a
    /// layer rig's slots back from its layer names.
    pub fn slot_for(&self, name: &str) -> Option<usize> {
        self.slot_named(name)
            .or_else(|| canonical_slot(name).and_then(|slot| self.slot_named(slot)))
    }

    /// Give every slot the limits `defaults` has for its name (relative to the
    /// rest angle); a slot the table does not name, or names as free, turns
    /// freely.
    pub fn apply_limits(&mut self, defaults: &JointLimitDefaults) {
        for slot in &mut self.slots {
            slot.limits = defaults.limits_for(&slot.name);
        }
    }

    /// Which way each slot points in the world, following the parent chain.
    ///
    /// Used to decide which end of a drawing is its *head*: a forearm drawn
    /// lying at some angle has two ends, and the one nearer the elbow is
    /// whichever end the pattern says the bone starts at.
    pub fn world_angles(&self) -> Vec<f64> {
        let mut out: Vec<f64> = Vec::with_capacity(self.slots.len());
        for slot in &self.slots {
            let angle = match slot.parent {
                // Parents are always earlier, so this is already computed.
                Some(p) => out.get(p).copied().unwrap_or(0.0) + slot.rest_angle,
                None => slot.rest_angle,
            };
            out.push(wrap_pi(angle));
        }
        out
    }

    /// Required slots that nothing has been dropped on, by name.
    ///
    /// `filled` is one flag per slot; a shorter list counts the rest as empty,
    /// which is what a half-built assignment looks like.
    pub fn missing_required(&self, filled: &[bool]) -> Vec<&str> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(i, slot)| slot.required && !filled.get(*i).copied().unwrap_or(false))
            .map(|(_, slot)| slot.name.as_str())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Default joint limits per slot.
// ---------------------------------------------------------------------------

/// **Joint limits per slot name**, relative to each bone's rest angle, in
/// degrees: `name → (mode, direction, min, max)`.
///
/// A table rather than code so it can be saved, edited and read back (the
/// user's own copy overrides [`JointLimitDefaults::anatomical`]).
///
/// # The director is the ground truth
///
/// The signs follow the director's performances (`buzz_act::perform`), not a
/// left/right mirror: every verb bends *both* knees the same way (negative),
/// so a mirrored limit would freeze one side of every walk; the elbows bend
/// either way (see the table). A limit that clamps a verb is wrong by
/// definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JointLimitDefaults {
    pub rows: BTreeMap<String, (RotationMode, Direction, f64, f64)>,
}

impl JointLimitDefaults {
    /// The built-in table for [`RigPattern::biped`] and
    /// [`RigPattern::biped_full`]. The hips are the root and carry the whole
    /// body's lean, so they turn freely; there is no neck or spine slot, so
    /// the head and chest carry those ranges.
    pub fn anatomical() -> Self {
        use Direction::{Both, CounterClockwise};
        use RotationMode::{Free, Limited};
        let table: [(&str, RotationMode, Direction, f64, f64); 17] = [
            ("Hips", Free, Both, 0.0, 0.0),
            ("Chest", Limited, Both, -30.0, 30.0),
            ("Head", Limited, Both, -40.0, 40.0),
            ("Shoulder L", Limited, Both, -170.0, 170.0),
            ("Shoulder R", Limited, Both, -170.0, 170.0),
            // Both ways, on both sides. The director solves its gestures'
            // arms toward hand targets on each rig (`buzz_act::perform::
            // solve_arm`), so which way an elbow bends depends on the gesture
            // and on which way round the rig is drawn — a shrug's forearm
            // folds out one way on the puppet and the other on a front-view
            // Cast rig. The right elbow also carries `buzz_ai`'s "wave":
            // +117° on the plain biped, +107° on the traced pujariFront test
            // character.
            ("Elbow L", Limited, Both, -150.0, 150.0),
            ("Elbow R", Limited, Both, -150.0, 150.0),
            ("Thigh L", Limited, Both, -120.0, 90.0),
            ("Thigh R", Limited, Both, -120.0, 90.0),
            ("Knee L", Limited, CounterClockwise, -150.0, 0.0),
            ("Knee R", Limited, CounterClockwise, -150.0, 0.0),
            ("Wrist L", Limited, Both, -80.0, 80.0),
            ("Wrist R", Limited, Both, -80.0, 80.0),
            ("Ankle L", Limited, Both, -45.0, 45.0),
            ("Ankle R", Limited, Both, -45.0, 45.0),
            ("Brow L", Limited, Both, -15.0, 15.0),
            ("Brow R", Limited, Both, -15.0, 15.0),
        ];
        Self {
            rows: table
                .into_iter()
                .map(|(name, mode, direction, min, max)| {
                    (name.to_string(), (mode, direction, min, max))
                })
                .collect(),
        }
    }

    /// The limits for `slot`, **relative to its rest angle, in radians**, or
    /// `None` for a free joint or one the table does not name. A one-way
    /// direction keeps only its side of the range.
    pub fn limits_for(&self, slot: &str) -> Option<JointLimits> {
        let (mode, direction, min, max) = *self.rows.get(slot)?;
        match mode {
            RotationMode::Free => None,
            RotationMode::Locked => Some(JointLimits::new(0.0, 0.0)),
            RotationMode::Limited => {
                let (mut min, mut max) = (min.min(max), min.max(max));
                match direction {
                    Direction::Both => {}
                    Direction::Clockwise => {
                        min = min.max(0.0);
                        max = max.max(0.0);
                    }
                    Direction::CounterClockwise => {
                        min = min.min(0.0);
                        max = max.min(0.0);
                    }
                }
                Some(JointLimits::new(min.to_radians(), max.to_radians()))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Display names: what an animator calls each biped slot.
// ---------------------------------------------------------------------------

/// `(slot name, display name)` for every `RigPattern::biped_full` slot.
const DISPLAY_NAMES: &[(&str, &str)] = &[
    ("Hips", "Hips"),
    ("Chest", "Torso"),
    ("Head", "Head"),
    ("Shoulder L", "Upper Arm L"),
    ("Elbow L", "Forearm L"),
    ("Shoulder R", "Upper Arm R"),
    ("Elbow R", "Forearm R"),
    ("Thigh L", "Thigh L"),
    ("Knee L", "Shin L"),
    ("Thigh R", "Thigh R"),
    ("Knee R", "Shin R"),
    ("Wrist L", "Hand L"),
    ("Wrist R", "Hand R"),
    ("Ankle L", "Foot L"),
    ("Ankle R", "Foot R"),
    ("Brow L", "Brow L"),
    ("Brow R", "Brow R"),
];

/// **The one alias table for layer-rig names**: the biped slot a layer name
/// means, whether it is the slot's own name ("Shoulder R"), its display
/// name ("Upper Arm R"), or the user's Animate convention ("leftArm",
/// "rightWrist" = forearm, "leftPalm" = hand, "body", "midCloth", typos such
/// as "righttThigh" / "rightThgh" and trailing digits included — see
/// [`body_part`]). Case-insensitive. A part with sides needs a side word in
/// the name. `None` for any other name. Everything that reads a layer rig
/// back from its layer names goes through this (usually via
/// [`RigPattern::slot_for`]), so rigs named any of these ways keep working.
pub fn canonical_slot(name: &str) -> Option<&'static str> {
    if let Some(slot) = exact_slot(name) {
        return Some(slot);
    }
    let stem = body_part(name)?;
    let sided = |left: &'static str, right: &'static str| match name_side(name) {
        Some(Side::Left) => Some(left),
        Some(Side::Right) => Some(right),
        _ => None,
    };
    match stem {
        "Hips" => Some("Hips"),
        "Chest" => Some("Chest"),
        "Head" => Some("Head"),
        "Shoulder" => sided("Shoulder L", "Shoulder R"),
        "Elbow" => sided("Elbow L", "Elbow R"),
        "Wrist" => sided("Wrist L", "Wrist R"),
        "Thigh" => sided("Thigh L", "Thigh R"),
        "Knee" => sided("Knee L", "Knee R"),
        "Ankle" => sided("Ankle L", "Ankle R"),
        "Brow" => sided("Brow L", "Brow R"),
        _ => None,
    }
}

/// A biped slot's own name or display name, case-insensitively ("wrist r",
/// "Upper Arm L"); `None` for anything else — the user's convention names
/// included. Repair Layer Rigs groups a rig's layers by this: two layers
/// that merely both read as a torso ("TorsoOldManSide1", "TorsoOldManSide2")
/// are two parts, not one.
pub fn exact_slot(name: &str) -> Option<&'static str> {
    let name = name.trim();
    DISPLAY_NAMES
        .iter()
        .find(|(slot, display)| slot.eq_ignore_ascii_case(name) || display.eq_ignore_ascii_case(name))
        .map(|(slot, _)| *slot)
}

/// The side a layer name says, case-insensitively: "left"/"lef" or a lone
/// "l"/"lt"/"lft" word, "right"/"righ" or a lone "r"/"rt"/"rgt" word.
/// `None` when it says neither, or both.
pub fn name_side(name: &str) -> Option<Side> {
    let squashed = squash(name);
    let words = tokens(name);
    let word = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    let left = squashed.contains("lef") || word(&["l", "lt", "lft"]);
    let right = squashed.contains("righ") || word(&["r", "rt", "rgt"]);
    match (left, right) {
        (true, false) => Some(Side::Left),
        (false, true) => Some(Side::Right),
        _ => None,
    }
}

/// **What a layer name says it is, in the user's Animate convention**
/// (`docs/AI_LAYER_RIG_AND_MOTION_COPY_PLAN.md` Part N), as a `biped_full`
/// slot stem: "Hips", "Chest", "Head", "Shoulder", "Elbow", "Wrist",
/// "Thigh", "Knee", "Ankle", "Brow". Substring matching on the lowercased
/// letters and digits, as the user's own JSFL commands match, so camel case,
/// trailing digits and typos all read: `leftArm1` / `sideArm` → Shoulder;
/// `rightWrist` / `backWrisat1a` → **Elbow** (the forearm: that is what the
/// convention calls a wrist); `leftPalm` / `leftPlam2` → **Wrist** (the hand);
/// `righttThigh` / `rightThgh` → Thigh; `rightShin1` → Knee; `rightFoot` /
/// `shoe` → Ankle; `body` / `tttorso1` / `torsoTop` → Chest; `midCloth1` /
/// `backCloth` / `lowTorso` → Hips; `frontHEadman` / `manFaceFront1` → Head.
/// Face pieces (eye, mouth, nose, hair, base, mask...) say nothing (only a
/// brow is a slot), nor do shadow, sound, an "Extra" piece, or an old
/// "<Slot> · <role>" extra.
pub fn body_part(name: &str) -> Option<&'static str> {
    if name.contains('\u{b7}') {
        return None;
    }
    let s = squash(name);
    if s.is_empty() {
        return None;
    }
    let words = tokens(name);
    let has = |list: &[&str]| list.iter().any(|w| s.contains(w));
    let word = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    if has(&["extra", "shadow", "sound"]) {
        return None;
    }
    if has(&["brow"]) {
        return Some("Brow");
    }
    if has(&[
        "eye", "mouth", "nose", "hair", "moustache", "mustache", "jaw", "teeth", "tongue", "mask", "ball",
        "base", "lips", "pupil",
    ]) || word(&["lip"])
    {
        return None;
    }
    if has(&["palm", "plam", "hand", "fist", "finger", "thumb"]) {
        return Some("Wrist");
    }
    if has(&["wrist", "wrisat", "forearm", "elbow", "lowerarm"]) {
        return Some("Elbow");
    }
    if has(&["shin", "knee", "calf", "lowerleg"]) {
        return Some("Knee");
    }
    if has(&["foot", "feet", "shoe", "boot", "ankle", "toe"]) {
        return Some("Ankle");
    }
    if has(&["thigh", "thgh", "thig", "upperleg"]) || (s.contains("leg") && !s.contains("legs")) {
        return Some("Thigh");
    }
    if has(&["arm", "shoulder", "bicep", "sleeve"]) {
        return Some("Shoulder");
    }
    if has(&["head", "face", "skull"]) {
        return Some("Head");
    }
    let torso = has(&["torso", "torss"]);
    if has(&["midcloth", "cloth", "hips", "pelvis", "waist", "dhoti", "lungi", "skirt", "pant", "trouser"])
        || word(&["hip"])
        || (torso && word(&["low", "lower", "bottom", "bot"]))
    {
        return Some("Hips");
    }
    if torso || has(&["chest", "body"]) {
        return Some("Chest");
    }
    None
}

/// **Shift+R, the user's Rename Face Layers rule** (`renameHeadLayers.jsfl`,
/// `characterSetup.jsfl` step 4), for one layer name: brow →
/// `leftEyebrow` / `rightEyebrow` / `eyebrow`; eye white → `leftEyeWhite` /
/// `rightEyeWhite` / `eyeWhite`; ball → `eyeBall`; jaw / mouth / lips →
/// `mouth`. `None` for anything else (eyeLid, eyeMask, nose, hair, base keep
/// their names) and for "Layer…", "sound" and blank names. Compare the answer
/// case-insensitively: `leftEyeBrow` already is `leftEyebrow`.
pub fn face_layer_name(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    if lower.trim().is_empty() || lower.starts_with("layer") || lower.contains("sound") {
        return None;
    }
    let side = name_side(name).or_else(|| {
        let left = lower.contains("l_") || lower.contains("_l");
        let right = lower.contains("r_") || lower.contains("_r");
        match (left, right) {
            (true, false) => Some(Side::Left),
            (false, true) => Some(Side::Right),
            _ => None,
        }
    });
    let sided = |left: &str, right: &str, either: &str| {
        Some(match side {
            Some(Side::Left) => left.to_string(),
            Some(Side::Right) => right.to_string(),
            _ => either.to_string(),
        })
    };
    if lower.contains("brow") {
        return sided("leftEyebrow", "rightEyebrow", "eyebrow");
    }
    if lower.contains("eyewhite") || (lower.contains("eye") && lower.contains("white")) {
        return sided("leftEyeWhite", "rightEyeWhite", "eyeWhite");
    }
    if lower.contains("ball") {
        return Some("eyeBall".into());
    }
    if lower.contains("jaw") || lower.contains("mouth") || lower.contains("lips") || tokens(name).iter().any(|w| w == "lip") {
        return Some("mouth".into());
    }
    None
}

// ---------------------------------------------------------------------------
// Naming profiles: what AI Rig as Layers and Rename Body / Face Layers call
// each body part, and what each part follows
// (docs/AI_LAYER_RIG_AND_MOTION_COPY_PLAN.md N1).
// ---------------------------------------------------------------------------

/// One body part in a [`NamingProfile`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfilePart {
    /// A `biped_full` slot stem: "Hips", "Chest", "Head", "Shoulder",
    /// "Elbow", "Wrist", "Thigh", "Knee", "Ankle" or "Brow".
    pub part: String,
    /// The layer name on the left and on the right (the same for a part with
    /// one side).
    pub left: String,
    pub right: String,
    /// The part it follows, on the same side; `None` is a root.
    #[serde(default)]
    pub parent: Option<String>,
}

/// **A naming convention as data**: each body part's layer name and parent,
/// and the conventions around them. Built in: [`Self::animate_convention`]
/// (the default) and [`Self::descriptive`]; `rig_naming.json` beside the
/// workspace can override (see [`Self::with_overrides`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamingProfile {
    pub name: String,
    pub parts: Vec<ProfilePart>,
    /// Left and right are the **screen's** (a side word in the name wins,
    /// then the screen side of the limb's root joint, then on a side view the
    /// front-most limb is the right one); `false`: the slot's own L/R.
    #[serde(default)]
    pub screen_sides: bool,
    /// Every part with no parent follows the rig's first root, so the
    /// character moves as one (`false`: roots stand alone).
    #[serde(default)]
    pub join_roots: bool,
    /// The folder the leg layers go in, when they are together below the
    /// body (`None`: no folder).
    #[serde(default)]
    pub legs_folder: Option<String>,
    /// A shadow's layer name; shadow pieces are left out of the rig.
    /// `None`: a shadow is an ordinary piece.
    #[serde(default)]
    pub shadow: Option<String>,
    /// Face pieces take the Shift+R names ([`face_layer_name`]), inside the
    /// head symbol too; other kept names stay as written. `false`: kept names
    /// have their side written "Eye R".
    #[serde(default)]
    pub face_names: bool,
    /// The name of an extra piece with no name of its own: `{name}` is its
    /// part's layer name, `{n}` counts from 1.
    #[serde(default = "default_extra")]
    pub extra: String,
    /// The part an upper-body extra goes to and follows — a piece whose name
    /// says no body part (gamcha, scarf) put on the torso or an upper arm,
    /// above the hips. `None`: it stays where it was put.
    #[serde(default)]
    pub upper_extras: Option<String>,
}

fn default_extra() -> String {
    "{name} Extra {n}".into()
}

impl NamingProfile {
    /// **"My Animate convention"**, the user's own (sampleNaming.fla and
    /// their Animate commands): `head`, `body`, `leftArm`, `leftWrist`
    /// (forearm), `leftPalm` (hand), `midCloth`, `leftThigh`, `leftShin`,
    /// `leftFoot`, `leftEyebrow`; screen sides; body, midCloth and the thighs
    /// are roots; legs in a `legs` folder; `shadow` left out.
    pub fn animate_convention() -> Self {
        let part = |part: &str, left: &str, right: &str, parent: Option<&str>| ProfilePart {
            part: part.into(),
            left: left.into(),
            right: right.into(),
            parent: parent.map(str::to_string),
        };
        Self {
            name: "My Animate convention".into(),
            parts: vec![
                part("Hips", "midCloth", "midCloth", None),
                part("Chest", "body", "body", None),
                part("Head", "head", "head", Some("Chest")),
                part("Shoulder", "leftArm", "rightArm", Some("Chest")),
                part("Elbow", "leftWrist", "rightWrist", Some("Shoulder")),
                part("Wrist", "leftPalm", "rightPalm", Some("Elbow")),
                part("Thigh", "leftThigh", "rightThigh", None),
                part("Knee", "leftShin", "rightShin", Some("Thigh")),
                part("Ankle", "leftFoot", "rightFoot", Some("Knee")),
                part("Brow", "leftEyebrow", "rightEyebrow", Some("Head")),
            ],
            screen_sides: true,
            join_roots: false,
            legs_folder: Some("legs".into()),
            shadow: Some("shadow".into()),
            face_names: true,
            extra: "{name}Extra{n}".into(),
            upper_extras: Some("Chest".into()),
        }
    }

    /// **"Descriptive"**: the display names ("Upper Arm R", "Forearm R",
    /// "Hand R", "Torso") on the biped's own parents, every root following the
    /// hips, sides as the slots have them.
    pub fn descriptive() -> Self {
        let pattern = RigPattern::biped_full();
        let mut parts: Vec<ProfilePart> = Vec::new();
        for slot in &pattern.slots {
            let stem = slot.name.trim_end_matches(" L").trim_end_matches(" R").to_string();
            if parts.iter().any(|p| p.part == stem) {
                continue;
            }
            let display = |side: &str| {
                let name = if stem == slot.name { stem.clone() } else { format!("{stem} {side}") };
                display_slot_name(&name).to_string()
            };
            let parent = slot.parent.map(|p| {
                let name = &pattern.slots[p].name;
                name.trim_end_matches(" L").trim_end_matches(" R").to_string()
            });
            parts.push(ProfilePart { left: display("L"), right: display("R"), part: stem, parent });
        }
        Self {
            name: "Descriptive".into(),
            parts,
            screen_sides: false,
            join_roots: true,
            legs_folder: None,
            shadow: None,
            face_names: false,
            extra: default_extra(),
            upper_extras: None,
        }
    }

    /// The built-in profiles, the default first.
    pub fn builtin() -> Vec<NamingProfile> {
        vec![Self::animate_convention(), Self::descriptive()]
    }

    /// The built-in profiles with `overrides` (from `rig_naming.json`) laid
    /// over them: one with a built-in's name replaces it, any other is added,
    /// and the first override becomes the default (first in the list).
    pub fn with_overrides(overrides: Vec<NamingProfile>) -> Vec<NamingProfile> {
        let mut all = Self::builtin();
        for (index, profile) in overrides.into_iter().enumerate() {
            all.retain(|p| !p.name.eq_ignore_ascii_case(&profile.name));
            all.insert(index.min(all.len()), profile);
        }
        all
    }

    fn part(&self, stem: &str) -> Option<&ProfilePart> {
        self.parts.iter().find(|p| p.part.eq_ignore_ascii_case(stem))
    }

    /// The layer name for a `biped_full` slot ("Shoulder L") on `side`
    /// (`None`: the slot's own side). A slot the profile does not name keeps
    /// its display name.
    pub fn layer_name(&self, slot: &str, side: Option<Side>) -> String {
        let stem = slot.trim_end_matches(" L").trim_end_matches(" R");
        let side = side.unwrap_or(if slot.ends_with(" L") {
            Side::Left
        } else if slot.ends_with(" R") {
            Side::Right
        } else {
            Side::Either
        });
        match self.part(stem) {
            Some(part) if side == Side::Right => part.right.clone(),
            Some(part) => part.left.clone(),
            None => display_slot_name(slot).to_string(),
        }
    }

    /// The parent of every slot of `pattern` by this profile's table, for
    /// the layer rig builder: a part's parent on the slot's own side.
    pub fn parents(&self, pattern: &RigPattern) -> Vec<Option<usize>> {
        pattern
            .slots
            .iter()
            .map(|slot| {
                let stem = slot.name.trim_end_matches(" L").trim_end_matches(" R");
                let Some(part) = self.part(stem) else {
                    return slot.parent;
                };
                let parent = part.parent.as_deref()?;
                let suffix = &slot.name[stem.len()..];
                pattern
                    .slot_named(&format!("{parent}{suffix}"))
                    .or_else(|| pattern.slot_named(parent))
            })
            .collect()
    }

    /// The name of an extra piece of the part named `name`.
    pub fn extra_name(&self, name: &str, n: usize) -> String {
        self.extra.replace("{name}", name).replace("{n}", &n.to_string())
    }

    /// **Rename Body Layers** (Ctrl+R in the user's Animate) for one layer
    /// name: the profile's name for the part it names ([`canonical_slot`] /
    /// [`body_part`], its own side), the shadow name for a shadow. `None` for
    /// a name that says no part (or a part with sides but no side), for
    /// "Layer…" and "sound" names, and when the name already is the answer
    /// (case-insensitively).
    pub fn body_layer_name(&self, name: &str) -> Option<String> {
        let lower = name.trim().to_lowercase();
        if lower.is_empty() || lower.starts_with("layer") || lower.contains("sound") {
            return None;
        }
        let new = if let Some(shadow) = self.shadow.as_ref().filter(|_| lower.contains("shadow")) {
            shadow.clone()
        } else {
            let slot = canonical_slot(name)?;
            self.layer_name(slot, None)
        };
        (!new.eq_ignore_ascii_case(name.trim())).then_some(new)
    }
}

/// The display name of a biped slot ("Shoulder R" → "Upper Arm R"); a name
/// that is not a biped slot comes back as it is.
pub fn display_slot_name(slot: &str) -> &str {
    DISPLAY_NAMES
        .iter()
        .find(|(s, _)| *s == slot)
        .map_or(slot, |(_, display)| display)
}

// ---------------------------------------------------------------------------
// Matching layer names to slots.
// ---------------------------------------------------------------------------

/// Break a name into lowercase words.
///
/// The four spellings that actually turn up in imported artwork — `leftArm`,
/// `L_arm`, `arm_left` and `arm.L` — differ only in *how* they separate the
/// words, so separating them is the whole job. Case changes count as
/// separators alongside punctuation, and so does the boundary between letters
/// and digits, because `arm2` is an arm and not something else.
pub fn tokens(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut previous: Option<char> = None;

    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            previous = None;
            continue;
        }
        // `armL` and `arm2` both start a new word here. `LArm` does not start
        // one at the `A`: a run of capitals is one word until a lowercase
        // letter follows it, which is what `IKHandle` needs and what `Arm`
        // must not be broken by.
        let boundary = previous.is_some_and(|p| {
            (c.is_uppercase() && p.is_lowercase()) || (c.is_ascii_digit() != p.is_ascii_digit())
        });
        if boundary && !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        word.extend(c.to_lowercase());
        previous = Some(c);
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// Which side a name says it is, if it says at all.
fn side_of(tokens: &[String]) -> Option<Side> {
    tokens.iter().find_map(|t| match t.as_str() {
        "l" | "lt" | "lft" | "left" => Some(Side::Left),
        "r" | "rt" | "rgt" | "right" => Some(Side::Right),
        _ => None,
    })
}

/// An alias with everything but its letters and digits taken out.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

impl Slot {
    /// How well a name fits this slot, or `None` if it does not.
    ///
    /// Matching is on **whole words**, never on substrings, which is what
    /// keeps `forearm` off the upper arm: the alias `arm` is compared against
    /// the words `["l", "forearm"]` and against runs of them joined together,
    /// and `forearm` is not `arm` in either. The longest alias that matches
    /// wins, so a slot never has to list its aliases in a careful order.
    pub fn score(&self, words: &[String], side: Option<Side>) -> Option<u32> {
        // **A side that disagrees is a refusal, not a penalty.**
        match self.side {
            Side::Left | Side::Right => {
                if side != Some(self.side) {
                    return None;
                }
            }
            Side::Either => {}
        }

        let mut best: Option<u32> = None;
        for alias in &self.aliases {
            let wanted = squash(alias);
            if wanted.is_empty() {
                continue;
            }
            // Every contiguous run of words, joined: this is what lets one
            // alias — "upper arm" — match `upperArm`, `upper_arm` and
            // `UPPER ARM` without three entries in the table.
            let matched = (0..words.len()).any(|start| {
                let mut joined = String::new();
                words[start..].iter().any(|w| {
                    joined.push_str(w);
                    joined.len() <= wanted.len() && joined == wanted
                })
            });
            if matched {
                let score = wanted.len() as u32 * 10;
                best = Some(best.map_or(score, |b| b.max(score)));
            }
        }

        // A name that is *only* the part and its side — `L_forearm` — is a
        // surer thing than one with other words in it, and this breaks ties
        // towards the layer whose name was written for exactly this slot.
        let spare = words.len().saturating_sub(usize::from(side.is_some()));
        best.map(|score| if spare <= 1 { score + 5 } else { score })
    }
}

/// Which drawing goes in which slot, worked out from their names.
///
/// Returns one entry per slot: the index into `names` of the part that should
/// fill it, or `None` for a slot nothing convincing was found for.
///
/// # Why greedy, best first
///
/// Two layers can want the same slot — `arm` and `arm copy` both look like an
/// upper arm — and one layer can suit two slots. Taking the strongest pairing
/// first and striking out both sides of it is not the globally optimal
/// assignment, but it is the one that is *explicable*: the best-named layer
/// gets the slot it names, every time, whatever else is in the file. An
/// animator who can predict what the button will do will use it.
pub fn match_parts(pattern: &RigPattern, names: &[String]) -> Vec<Option<usize>> {
    let parsed: Vec<(Vec<String>, Option<Side>)> = names
        .iter()
        .map(|name| {
            let words = tokens(name);
            let side = side_of(&words);
            (words, side)
        })
        .collect();

    let mut candidates: Vec<(u32, usize, usize)> = Vec::new();
    for (part, (words, side)) in parsed.iter().enumerate() {
        for (index, slot) in pattern.slots.iter().enumerate() {
            if let Some(score) = slot.score(words, *side) {
                candidates.push((score, part, index));
            }
        }
    }

    // Strongest first; ties settled by the order the parts and slots came in,
    // so the same file always rigs the same way.
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    let mut filled = vec![None; pattern.slots.len()];
    let mut taken = vec![false; names.len()];
    for (_, part, index) in candidates {
        if taken[part] || filled[index].is_some() {
            continue;
        }
        taken[part] = true;
        filled[index] = Some(part);
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_four_spellings_of_a_left_arm_all_break_into_the_same_words() {
        assert_eq!(tokens("leftArm"), ["left", "arm"]);
        assert_eq!(tokens("L_arm"), ["l", "arm"]);
        assert_eq!(tokens("arm_left"), ["arm", "left"]);
        assert_eq!(tokens("arm.L"), ["arm", "l"]);
        assert_eq!(tokens("ARM 2"), ["arm", "2"]);
    }

    /// All four of these have to land on the same slot, or the button is a
    /// lottery and nobody presses it twice.
    #[test]
    fn every_spelling_of_a_left_arm_lands_on_the_left_shoulder() {
        let pattern = RigPattern::biped();
        let shoulder = pattern.slot_named("Shoulder L").expect("a left shoulder");

        for spelling in ["leftArm", "L_arm", "arm_left", "arm.L", "Left Arm"] {
            let filled = match_parts(&pattern, &names(&[spelling]));
            assert_eq!(
                filled[shoulder],
                Some(0),
                "{spelling} did not reach the left shoulder"
            );
        }
    }

    /// The whole reason matching is on words: `arm` is inside `forearm`, and a
    /// substring match would put the forearm on the shoulder.
    #[test]
    fn a_forearm_is_not_an_arm() {
        let pattern = RigPattern::biped();
        let filled = match_parts(&pattern, &names(&["L_forearm", "L_arm"]));

        assert_eq!(filled[pattern.slot_named("Elbow L").unwrap()], Some(0));
        assert_eq!(filled[pattern.slot_named("Shoulder L").unwrap()], Some(1));
    }

    /// Swapped arms are the failure worth refusing outright.
    #[test]
    fn a_left_part_never_reaches_a_right_slot() {
        let pattern = RigPattern::biped();
        let filled = match_parts(&pattern, &names(&["arm_L"]));

        assert_eq!(filled[pattern.slot_named("Shoulder L").unwrap()], Some(0));
        assert_eq!(filled[pattern.slot_named("Shoulder R").unwrap()], None);
    }

    /// A drawing that does not say which side it is stays where the animator
    /// can see it is unassigned, rather than being guessed onto one arm.
    #[test]
    fn a_part_with_no_side_does_not_fill_a_sided_slot() {
        let pattern = RigPattern::biped();
        let filled = match_parts(&pattern, &names(&["arm"]));
        assert!(filled.iter().all(Option::is_none), "{filled:?}");
    }

    #[test]
    fn a_tidy_export_rigs_itself_completely() {
        let pattern = RigPattern::biped();
        let filled = match_parts(
            &pattern,
            &names(&[
                "hips",
                "torso",
                "head",
                "L_upperArm",
                "L_forearm",
                "R_upperArm",
                "R_forearm",
                "L_thigh",
                "L_shin",
                "R_thigh",
                "R_shin",
            ]),
        );

        let full: Vec<bool> = filled.iter().map(Option::is_some).collect();
        assert!(
            pattern.missing_required(&full).is_empty(),
            "still empty: {:?}",
            pattern.missing_required(&full)
        );
    }

    #[test]
    fn one_drawing_fills_one_slot() {
        let pattern = RigPattern::biped();
        // Two layers that both look like a left upper arm.
        let filled = match_parts(&pattern, &names(&["L_arm", "L_arm copy"]));

        let used: Vec<usize> = filled.iter().flatten().copied().collect();
        let mut sorted = used.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(used.len(), sorted.len(), "a drawing was used twice");
    }

    #[test]
    fn nothing_at_all_matches_nothing() {
        let pattern = RigPattern::biped();
        let filled = match_parts(&pattern, &names(&["Layer 1", "background", "sky"]));
        assert!(filled.iter().all(Option::is_none), "{filled:?}");
    }

    /// Parents come first in every pattern, which is what lets a pattern be
    /// walked in one pass — and what `Armature::push` would otherwise silently
    /// correct by orphaning the bone.
    #[test]
    fn every_pattern_names_its_parents_before_its_children() {
        for pattern in RigPattern::builtin() {
            for (index, slot) in pattern.slots.iter().enumerate() {
                if let Some(parent) = slot.parent {
                    assert!(
                        parent < index,
                        "{}: {} hangs off a later slot",
                        pattern.name,
                        slot.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_pattern_can_be_found_again_by_the_name_it_stores() {
        for pattern in RigPattern::builtin() {
            let found = RigPattern::named(&pattern.name).expect("a built-in pattern");
            assert_eq!(found, pattern);
        }
        assert!(RigPattern::named("Centaur").is_none());
    }

    #[test]
    fn world_angles_follow_the_parent_chain() {
        let pattern = RigPattern::biped();
        let angles = pattern.world_angles();
        // The chest hangs off the hips at no angle of its own, so it points
        // the same way: up.
        assert!((angles[0] - UP).abs() < 1e-9);
        assert!((angles[1] - UP).abs() < 1e-9);
        // A left thigh is a root pointing down and a little outwards.
        assert!(angles[7] > 0.0, "a thigh should point down the screen");
    }

    /// The eleven standard slots keep their numbers: a performance addresses
    /// bones by index, so `biped_full` may only append.
    #[test]
    fn biped_full_is_the_biped_plus_six_appended_slots() {
        let base = RigPattern::biped();
        let full = RigPattern::biped_full();
        assert_eq!(full.name, "Biped Full");
        assert_eq!(full.slots.len(), 17);
        for (a, b) in base.slots.iter().zip(full.slots.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.parent, b.parent);
            assert_eq!(a.rest_angle, b.rest_angle);
            assert_eq!(a.rest_len, b.rest_len);
            assert_eq!(a.side, b.side);
            assert_eq!(a.required, b.required);
        }
        let appended: Vec<(&str, Option<usize>, Side)> = full.slots[11..]
            .iter()
            .map(|s| (s.name.as_str(), s.parent, s.side))
            .collect();
        assert_eq!(
            appended,
            [
                ("Wrist L", Some(4), Side::Left),
                ("Wrist R", Some(6), Side::Right),
                ("Ankle L", Some(8), Side::Left),
                ("Ankle R", Some(10), Side::Right),
                ("Brow L", Some(2), Side::Left),
                ("Brow R", Some(2), Side::Right),
            ]
        );
        assert!(full.slots[11..].iter().all(|s| !s.required));
        // `biped` itself is untouched: its elbow and knee still answer to hand and foot.
        assert!(base.slots[4].aliases.iter().any(|a| a == "hand"));
        assert!(base.slots[8].aliases.iter().any(|a| a == "foot"));
        assert_eq!(base.slots.len(), 11);
    }

    #[test]
    fn biped_full_sends_hands_feet_and_brows_to_their_own_slots() {
        let pattern = RigPattern::biped_full();
        let filled = match_parts(
            &pattern,
            &names(&[
                "L_forearm",
                "L_hand",
                "R_shin",
                "R_foot",
                "Left Eyebrow",
                "R_brow",
            ]),
        );
        let slot = |n: &str| pattern.slot_named(n).unwrap();
        assert_eq!(filled[slot("Elbow L")], Some(0));
        assert_eq!(filled[slot("Wrist L")], Some(1));
        assert_eq!(filled[slot("Knee R")], Some(2));
        assert_eq!(filled[slot("Ankle R")], Some(3));
        assert_eq!(filled[slot("Brow L")], Some(4));
        assert_eq!(filled[slot("Brow R")], Some(5));
    }

    /// Elbows and knees bend one way — the way the director bends them — and
    /// the same way on both sides: no left/right mirror (see
    /// `JointLimitDefaults`).
    #[test]
    fn elbows_and_knees_bend_one_way_on_both_sides() {
        let pattern = RigPattern::biped();
        let relative = |name: &str| {
            let limits = pattern.slots[pattern.slot_named(name).unwrap()]
                .limits
                .unwrap_or_else(|| panic!("{name} has no limits"));
            (limits.min.to_degrees(), limits.max.to_degrees())
        };
        for name in ["Knee L", "Knee R"] {
            let (min, max) = relative(name);
            assert!((min + 150.0).abs() < 1e-9 && max.abs() < 1e-9, "{name}: {min}..{max}");
            let limits = JointLimits::new(min.to_radians(), max.to_radians());
            assert_eq!(limits.direction(0.0), Direction::CounterClockwise, "{name}");
        }
        for name in ["Elbow L", "Elbow R"] {
            let (min, max) = relative(name);
            assert!((min + 150.0).abs() < 1e-9, "{name}: {min}..{max}");
            assert!(max >= 0.0, "{name} reaches no further than rest: {max}");
        }
        // The right elbow reaches further the other way, for the wave.
        assert!(relative("Elbow R").1 >= 117.0, "the wave needs +117°: {:?}", relative("Elbow R"));
        assert_eq!(relative("Knee L"), relative("Knee R"), "knees mirrored");
        assert!(pattern.slots[0].limits.is_none(), "the root hips turn freely");

        // Every slot of the full biped is named in the table, so nothing is
        // left free by a typo.
        let full = RigPattern::biped_full();
        for slot in &full.slots[1..] {
            assert!(slot.limits.is_some(), "{} is free", slot.name);
        }
        // And each range contains the rest angle, so a fresh rig is legal.
        for limits in full.slots.iter().filter_map(|s| s.limits) {
            assert!(limits.min <= 0.0 && limits.max >= 0.0, "{limits:?}");
        }
    }

    #[test]
    fn missing_required_names_the_empty_slots() {
        let pattern = RigPattern::prop();
        let missing = pattern.missing_required(&[true, false, false]);
        // The tip is optional; the arm is not.
        assert_eq!(missing, ["Arm"]);
    }
}
