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

use crate::{JointLimits, wrap_pi};
use serde::{Deserialize, Serialize};

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
        Self {
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
        }
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
                    &["primaries", "wing tip", "outer wing", "lower wing", "feathers"],
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
                    &["primaries", "wing tip", "outer wing", "lower wing", "feathers"],
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

    #[test]
    fn missing_required_names_the_empty_slots() {
        let pattern = RigPattern::prop();
        let missing = pattern.missing_required(&[true, false, false]);
        // The tip is optional; the arm is not.
        assert_eq!(missing, ["Arm"]);
    }
}
