//! **How a character is cut into pieces**, as data: per body part, what the
//! piece carries, the shape of its end at the joint, how far it reaches under
//! its neighbour, what it hides at rest and where its pivot sits.
//!
//! Sits beside [`crate::pattern::NamingProfile`] (names, parents, folder and
//! stacking): that one says what the layers are called, this one says where
//! the scissors go. Part keys are the naming profile's (`Head`, `Chest`,
//! `Shoulder` = upper arm, `Elbow` = forearm, `Wrist` = hand, `Hips` =
//! midCloth, `Thigh`, `Knee` = shin, `Ankle` = foot) plus `Scarf`, `LowTorso`
//! (body2: the shirt below the waist, a saree skirt) and `Pallu`.
//!
//! # Units
//!
//! * Bone length `L`: from the piece's joint with its parent to its joint with
//!   its child (a hand or foot: to its far tip). Positions along the bone are
//!   fractions of `L`, measured from the joint, positive towards the child.
//! * The joint is the centre of the piece's overlap with its parent. On the
//!   user's distal limbs the pivot sits there (within 0.1 of a limb width).
//! * Limb width `w`: the piece's mean width across the middle of the bone.
//! * Pixels are document units at the size the characters were drawn
//!   (about 1150 units tall).
//!
//! [`CutStyle::animate_convention`] is measured, not designed: see
//! `docs/STYLE_CUT_AND_OUTPAINT_PLAN.md` § S0 findings for the table, the
//! characters it came from and the spread behind each median.

use serde::{Deserialize, Serialize};

/// The shape of a piece's drawn end at a joint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EndShape {
    /// A rounded cap, about a half circle across the limb (roundness ≈ 1).
    #[default]
    RoundCap,
    /// A flattened round (roundness 0.25–0.6).
    Soft,
    /// A straight cut across the bone (roundness < 0.25).
    Straight,
}

/// Where the piece sits in the stack relative to the part it follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Stacking {
    /// Below its parent, its end tucked under the parent's end.
    #[default]
    UnderParent,
    /// Above its parent, its end laid over the parent's.
    OverParent,
    /// Follows nothing (body, midCloth, thighs).
    Root,
}

/// Which part of the piece is hidden at rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HiddenAt {
    /// The end at the joint with its parent.
    #[default]
    Proximal,
    /// The far end, under its child.
    Distal,
    /// Its sides (a torso under the arms).
    Sides,
    /// Nothing is hidden.
    Nowhere,
}

/// Where the transformation point goes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum PivotPlace {
    /// The centre of the overlap with the parent: the joint.
    #[default]
    OverlapCentre,
    /// On the bone, this many limb widths in from the drawn proximal tip.
    FromTip(f64),
    /// On the bone at this fraction of its length from the proximal joint.
    AlongBone(f64),
}

/// Where the arm chains lie against the body in one view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ArmsVsBody {
    /// Both arms over the body (front views).
    #[default]
    Over,
    /// Both arms under the body (back and three-quarter-back views).
    Under,
    /// The near arm over the body, the far arm under it (side views).
    NearOverFarUnder,
}

/// **The stack in one view**, measured: the majority over the user's
/// characters drawn in that view. Front, side (three-quarter front), back,
/// back34 (three-quarter back).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewStacking {
    /// `"front"`, `"side"`, `"back"` or `"back34"`.
    pub view: String,
    /// How many of the user's character views this came from.
    pub samples: u32,
    pub arms: ArmsVsBody,
    /// Forearm against the upper arm.
    pub elbow: Stacking,
    /// Hand against the forearm.
    pub wrist: Stacking,
    /// Shin against the thigh.
    pub knee: Stacking,
    /// Foot against the shin.
    pub ankle: Stacking,
    pub head_over_body: bool,
    /// midCloth over the thighs (dhoti). A trouser crotch piece lies under
    /// everything instead.
    pub mid_cloth_over_thighs: bool,
    /// The LowTorso (body2) over the Chest piece.
    pub low_torso_over_body: bool,
    /// A pallu over the body (it hangs behind it in front and side views).
    pub pallu_over_body: bool,
    /// The right leg (the character's) above the left one.
    pub right_leg_over_left: bool,
}

impl Default for ViewStacking {
    fn default() -> Self {
        Self {
            view: "front".into(),
            samples: 0,
            arms: ArmsVsBody::Over,
            elbow: Stacking::UnderParent,
            wrist: Stacking::UnderParent,
            knee: Stacking::UnderParent,
            ankle: Stacking::UnderParent,
            head_over_body: true,
            mid_cloth_over_thighs: true,
            low_torso_over_body: false,
            pallu_over_body: false,
            right_leg_over_left: true,
        }
    }
}

/// **Which piece a garment goes with**, for the garments past the dhoti
/// and half-sleeved shirt of the first study: the saree, trousers, shoes,
/// hair and full sleeves. Values are part keys of [`SlotCut::part`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GarmentRules {
    /// The saree's skirt (waist to feet, its border and the pleated back
    /// drape) is its own root piece.
    pub saree_skirt_with: String,
    /// The saree's drape across the chest goes with the blouse.
    pub saree_chest_drape_with: String,
    /// The pallu is its own piece, following the skirt; it hangs behind the
    /// body in front and side views and over it (under the head) in back
    /// views.
    pub pallu_piece: bool,
    /// Under a saree there are no legs: the feet follow the skirt and their
    /// ankles hide under its hem.
    pub saree_feet_follow: String,
    /// A split shirt: the top (collar to waist, Chest) follows the bottom
    /// (waist to hem, LowTorso), which is the root.
    pub shirt_split_at_waist: bool,
    /// Trousers have no midCloth: a dome crotch piece (`base1`) follows the
    /// LowTorso and lies under every leg piece, hidden at rest.
    pub trouser_crotch_piece: bool,
    /// A trouser leg: thigh from hip to knee, domed at both ends; the shin
    /// carries the lower leg to the hem (rolled cuff included) and a bare
    /// shin below it.
    pub trouser_thigh_domed_both_ends: bool,
    /// A shoe goes with the foot and its top band hides under the trouser
    /// hem (0.22-0.37 of the shoe).
    pub shoe_with: String,
    /// Hair, a bun or long hair stays in the head piece (a separate hair
    /// layer inside the head symbol at most); the back-view head is the hair.
    pub hair_with: String,
    /// A full sleeve: the upper arm carries the sleeve to the elbow; the
    /// forearm carries the cuff (rolled or not) and the bare arm below it.
    pub full_sleeve_cuff_with: String,
}

impl Default for GarmentRules {
    fn default() -> Self {
        Self {
            saree_skirt_with: "LowTorso".into(),
            saree_chest_drape_with: "Chest".into(),
            pallu_piece: true,
            saree_feet_follow: "LowTorso".into(),
            shirt_split_at_waist: true,
            trouser_crotch_piece: true,
            trouser_thigh_domed_both_ends: true,
            shoe_with: "Ankle".into(),
            hair_with: "Head".into(),
            full_sleeve_cuff_with: "Elbow".into(),
        }
    }
}

/// One body part's cut.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlotCut {
    /// The naming profile's part key.
    pub part: String,
    /// What the piece carries: skin and garment pieces, in words.
    pub carries: Vec<String>,
    /// The drawn end at the joint with its parent.
    pub end_shape: EndShape,
    /// 1 = a half circle of radius `w/2`, 0 = straight, below 0 concave.
    pub end_roundness: f64,
    /// The end at the joint is hidden under its neighbour at rest.
    pub tucked: bool,
    /// Share of the hidden outline that is drawn with its ink line
    /// (1 = the hidden end is drawn complete with its outline).
    pub hidden_outline: f64,
    /// The visible edge at the joint (the cut line), as a fraction of `L`
    /// past the joint (negative: before it).
    pub cut_pos: f64,
    /// The cut line's angle against the bone's perpendicular, degrees.
    pub cut_angle_deg: f64,
    /// How far the drawing reaches back past the joint, as a fraction of `L`.
    pub tip_beyond_joint: f64,
    /// The overlap with the parent along the bone, document units.
    pub overlap_px: f64,
    /// The same overlap as a fraction of the limb width.
    pub overlap_width: f64,
    /// Share of the piece's own drawing covered at rest.
    pub hidden_frac: f64,
    /// Which parts cover it, most first.
    pub hidden_under: Vec<String>,
    /// Which end the covered part is at.
    pub hidden_at: HiddenAt,
    /// Where the pivot sits.
    pub pivot: PivotPlace,
    /// Stack position relative to its parent.
    pub stacking: Stacking,
    /// Bone length over limb width (the piece's proportions).
    pub length_over_width: f64,
}

impl Default for SlotCut {
    fn default() -> Self {
        Self {
            part: String::new(),
            carries: Vec::new(),
            end_shape: EndShape::RoundCap,
            end_roundness: 1.0,
            tucked: true,
            hidden_outline: 1.0,
            cut_pos: 0.0,
            cut_angle_deg: 0.0,
            tip_beyond_joint: 0.0,
            overlap_px: 0.0,
            overlap_width: 0.5,
            hidden_frac: 0.0,
            hidden_under: Vec::new(),
            hidden_at: HiddenAt::Proximal,
            pivot: PivotPlace::OverlapCentre,
            stacking: Stacking::UnderParent,
            length_over_width: 2.0,
        }
    }
}

/// **A way of cutting a character**, per part, plus the garment rules that
/// decide which piece a sleeve, collar or shoe goes with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutStyle {
    pub name: String,
    pub slots: Vec<SlotCut>,
    /// The part a sleeve goes with: `"arm"` (split along the arm pieces,
    /// never left on the body) or `"body"`.
    pub sleeve_with: String,
    /// The part a cuff goes with (the sleeve's end): `"forearm"` for a full
    /// sleeve, `"upper_arm"` for a half sleeve; the hand starts at bare skin.
    pub cuff_with: String,
    /// The part the collar and neck go with: `"body"` or `"head"`.
    pub collar_with: String,
    /// The part a shoe or sandal goes with.
    pub shoe_with: String,
    /// The torso is drawn complete (fill and outline) under the arms.
    pub torso_complete_under_arms: bool,
    /// The thighs are drawn complete under midCloth and the torso's hem.
    pub thighs_complete_under_mid_cloth: bool,
    /// The head ends at the chin and jaw; the neck is the body's.
    pub head_includes_neck: bool,
    /// Front and three-quarter-front views: the arms lie over the body.
    pub arms_over_body_front: bool,
    /// Back and three-quarter-back views: the arms go under the body.
    pub arms_under_body_back: bool,
    /// The stack per view, measured (front, side, back, back34).
    pub views: Vec<ViewStacking>,
    /// Saree, trousers, shoes, hair and full sleeves.
    pub garments: GarmentRules,
}

impl Default for CutStyle {
    fn default() -> Self {
        Self::animate_convention()
    }
}

impl CutStyle {
    /// **"My Animate cut style"**: medians measured from the user's own
    /// characters, 21 views: sampleNaming.fla (man1Frontt manFront,
    /// manSideFront, manBack, manSideBack; pradhanOldMan manFront, manSide,
    /// man1Back1, back3-4Old), morecharacters.fla (man2 frontBody, sideBody,
    /// BackBody; mazdoor1, MazdoorSide, mazdoorBack1, MazdoorBack3-4;
    /// newman1 rameshFrontPose, rameshSide, rameshBackPose) and
    /// womanSample.fla (womanFront, womanSide, womanBack). End roundness,
    /// cut angles and the Head/Chest cut lines stay the S0 values where the
    /// S0b measure did not reproduce them on the S0 characters; the elbow is
    /// round by the user's choice. Lengths are fractions of the bone, widths
    /// of the limb.
    pub fn animate_convention() -> Self {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let slots = vec![
            SlotCut {
                part: "Head".into(),
                carries: s(&["face", "hair or turban (long hair and a bun too)", "ears"]),
                end_shape: EndShape::RoundCap,
                end_roundness: 1.04,
                tucked: false,
                hidden_outline: 1.0,
                cut_pos: -0.09,
                cut_angle_deg: 7.0,
                tip_beyond_joint: 0.13,
                overlap_px: 36.7,
                overlap_width: 0.30,
                hidden_frac: 0.0,
                hidden_under: s(&["Chest"]),
                hidden_at: HiddenAt::Proximal,
                pivot: PivotPlace::FromTip(0.07),
                stacking: Stacking::OverParent,
                length_over_width: 1.12,
            },
            SlotCut {
                part: "Chest".into(),
                carries: s(&[
                    "shirt body without sleeves",
                    "collar",
                    "neck",
                    "with a LowTorso: only the shirt above the waist (torsoTop), or the blouse with the saree's chest drape",
                ]),
                end_shape: EndShape::Straight,
                end_roundness: 0.17,
                tucked: false,
                hidden_outline: 1.0,
                cut_pos: -0.12,
                cut_angle_deg: 0.5,
                tip_beyond_joint: 0.13,
                overlap_px: 96.0,
                overlap_width: 0.48,
                hidden_frac: 0.12,
                hidden_under: s(&["Shoulder", "Head", "Scarf"]),
                hidden_at: HiddenAt::Sides,
                // At the waist: 13 of 21 views put it there (every new
                // character, and pradhanOldMan at its hem); man1Frontt 0.46.
                pivot: PivotPlace::AlongBone(0.03),
                stacking: Stacking::Root,
                length_over_width: 1.72,
            },
            SlotCut {
                part: "Scarf".into(),
                carries: s(&["gamcha / scarf over the shoulder"]),
                end_shape: EndShape::RoundCap,
                end_roundness: 1.67,
                tucked: false,
                hidden_outline: 1.0,
                cut_pos: 0.0,
                cut_angle_deg: 0.0,
                tip_beyond_joint: 0.0,
                overlap_px: 321.0,
                overlap_width: 4.69,
                hidden_frac: 0.0,
                hidden_under: Vec::new(),
                hidden_at: HiddenAt::Nowhere,
                // 0.44 widths below its top end, on the shoulder.
                pivot: PivotPlace::FromTip(0.44),
                stacking: Stacking::OverParent,
                length_over_width: 2.59,
            },
            SlotCut {
                part: "Shoulder".into(),
                carries: s(&["sleeve top (full sleeve)", "half sleeve with its rolled cuff and the elbow skin below it"]),
                end_shape: EndShape::RoundCap,
                end_roundness: 2.17,
                tucked: false,
                hidden_outline: 0.53,
                cut_pos: 0.07,
                cut_angle_deg: 45.0,
                tip_beyond_joint: 0.36,
                overlap_px: 112.0,
                overlap_width: 1.78,
                hidden_frac: 0.20,
                hidden_under: s(&["Chest", "Scarf", "Elbow"]),
                hidden_at: HiddenAt::Proximal,
                pivot: PivotPlace::FromTip(0.18),
                stacking: Stacking::OverParent,
                length_over_width: 1.90,
            },
            SlotCut {
                part: "Elbow".into(),
                carries: s(&["forearm: lower sleeve and cuff (full sleeve) or bare skin (half sleeve)"]),
                // The user chose round elbows (2026-10-22): man1Frontt's
                // round end (0.89), not the pradhan median (0.42).
                end_shape: EndShape::RoundCap,
                end_roundness: 0.89,
                tucked: true,
                hidden_outline: 0.67,
                cut_pos: 0.10,
                cut_angle_deg: 2.0,
                tip_beyond_joint: 0.10,
                overlap_px: 27.6,
                overlap_width: 0.60,
                hidden_frac: 0.08,
                hidden_under: s(&["Shoulder", "Wrist"]),
                hidden_at: HiddenAt::Proximal,
                pivot: PivotPlace::OverlapCentre,
                // Kept as S1 uses it. Over all 21 views the forearm lies
                // OVER the upper arm in 24 of 40 (front 9/12, side 12/12,
                // back 2/11): see `views`.
                stacking: Stacking::UnderParent,
                length_over_width: 2.75,
            },
            SlotCut {
                part: "Wrist".into(),
                carries: s(&["hand from the bare wrist"]),
                end_shape: EndShape::RoundCap,
                end_roundness: 0.94,
                tucked: true,
                hidden_outline: 0.66,
                cut_pos: 0.09,
                cut_angle_deg: 4.0,
                tip_beyond_joint: 0.12,
                overlap_px: 18.5,
                overlap_width: 0.50,
                // The median hand hides nothing: 23 of 40 lie over the
                // forearm (tucked ones hide 0.13).
                hidden_frac: 0.0,
                hidden_under: s(&["Elbow"]),
                hidden_at: HiddenAt::Proximal,
                pivot: PivotPlace::OverlapCentre,
                stacking: Stacking::UnderParent,
                length_over_width: 1.92,
            },
            SlotCut {
                part: "Hips".into(),
                carries: s(&[
                    "dhoti centre fold between the thighs",
                    "trousers: a dome crotch piece (base1) under everything, hidden at rest",
                ]),
                end_shape: EndShape::Straight,
                end_roundness: 0.21,
                tucked: true,
                hidden_outline: 1.0,
                cut_pos: 0.10,
                cut_angle_deg: 1.0,
                tip_beyond_joint: 0.13,
                overlap_px: 63.0,
                overlap_width: 1.20,
                hidden_frac: 0.29,
                hidden_under: s(&["Chest", "LowTorso", "Thigh"]),
                hidden_at: HiddenAt::Proximal,
                pivot: PivotPlace::FromTip(0.77),
                stacking: Stacking::Root,
                length_over_width: 3.94,
            },
            SlotCut {
                part: "Thigh".into(),
                carries: s(&[
                    "upper leg of the dhoti / trousers",
                    "trousers: hip to knee, domed at both ends",
                ]),
                end_shape: EndShape::RoundCap,
                end_roundness: 1.64,
                tucked: true,
                hidden_outline: 1.0,
                cut_pos: 0.18,
                cut_angle_deg: 48.5,
                tip_beyond_joint: 0.48,
                overlap_px: 175.0,
                overlap_width: 1.44,
                hidden_frac: 0.45,
                hidden_under: s(&["Chest", "LowTorso", "Hips"]),
                hidden_at: HiddenAt::Proximal,
                pivot: PivotPlace::FromTip(0.31),
                stacking: Stacking::Root,
                length_over_width: 1.29,
            },
            SlotCut {
                part: "Knee".into(),
                carries: s(&[
                    "lower leg of the dhoti / trousers (a rolled cuff included)",
                    "bare shin below the hem",
                ]),
                end_shape: EndShape::RoundCap,
                end_roundness: 1.08,
                tucked: true,
                hidden_outline: 0.66,
                cut_pos: 0.15,
                cut_angle_deg: 0.0,
                tip_beyond_joint: 0.15,
                overlap_px: 59.0,
                overlap_width: 0.68,
                hidden_frac: 0.25,
                hidden_under: s(&["Thigh", "Hips"]),
                hidden_at: HiddenAt::Proximal,
                pivot: PivotPlace::OverlapCentre,
                stacking: Stacking::UnderParent,
                length_over_width: 2.45,
            },
            SlotCut {
                part: "Ankle".into(),
                carries: s(&[
                    "foot with the ankle",
                    "shoe or sandal (a shoe's top band hides under the trouser hem)",
                ]),
                end_shape: EndShape::RoundCap,
                end_roundness: 1.30,
                tucked: true,
                hidden_outline: 1.0,
                cut_pos: 0.10,
                cut_angle_deg: 26.5,
                tip_beyond_joint: 0.18,
                overlap_px: 28.0,
                overlap_width: 0.48,
                hidden_frac: 0.13,
                hidden_under: s(&["Knee", "LowTorso"]),
                hidden_at: HiddenAt::Proximal,
                pivot: PivotPlace::OverlapCentre,
                stacking: Stacking::UnderParent,
                length_over_width: 1.48,
            },
            SlotCut {
                part: "LowTorso".into(),
                carries: s(&[
                    "the shirt below the waist (torsoBottom), its top under the shirt top",
                    "the saree skirt from the waist to the feet, with its border and back drape",
                ]),
                // 1.22: man2 straight-topped with round corners, the saree's
                // top a dome.
                end_shape: EndShape::RoundCap,
                end_roundness: 1.22,
                tucked: true,
                hidden_outline: 1.0,
                cut_pos: 0.18,
                cut_angle_deg: 0.0,
                tip_beyond_joint: 0.20,
                overlap_px: 80.6,
                overlap_width: 0.46,
                hidden_frac: 0.23,
                hidden_under: s(&["Chest", "Pallu"]),
                hidden_at: HiddenAt::Proximal,
                // At its hem (man2 0.88 of its length, the saree 1.0).
                pivot: PivotPlace::AlongBone(0.92),
                stacking: Stacking::Root,
                length_over_width: 1.56,
            },
            SlotCut {
                part: "Pallu".into(),
                carries: s(&["the saree's loose end, hanging from the shoulder"]),
                end_shape: EndShape::RoundCap,
                end_roundness: 2.0,
                tucked: false,
                hidden_outline: 1.0,
                cut_pos: 0.0,
                cut_angle_deg: 0.0,
                tip_beyond_joint: 0.67,
                overlap_px: 451.0,
                overlap_width: 5.34,
                // Front and side 0.68-0.77 hidden behind the body; back 0.
                hidden_frac: 0.68,
                hidden_under: s(&["LowTorso", "Chest", "Shoulder"]),
                hidden_at: HiddenAt::Proximal,
                // On the shoulder, 0.18 widths below its top end.
                pivot: PivotPlace::FromTip(0.18),
                stacking: Stacking::UnderParent,
                length_over_width: 2.76,
            },
        ];
        Self {
            name: "My Animate cut style".into(),
            slots,
            sleeve_with: "arm".into(),
            cuff_with: "forearm".into(),
            collar_with: "body".into(),
            shoe_with: "foot".into(),
            torso_complete_under_arms: true,
            thighs_complete_under_mid_cloth: true,
            head_includes_neck: false,
            arms_over_body_front: true,
            arms_under_body_back: true,
            views: Self::measured_views(),
            garments: GarmentRules::default(),
        }
    }

    /// The stack per view over the user's 21 character views (S0b): the
    /// majority in each view; ties keep the S0 convention (tucked).
    pub fn measured_views() -> Vec<ViewStacking> {
        use Stacking::{OverParent, UnderParent};
        vec![
            ViewStacking {
                view: "front".into(),
                samples: 6,
                arms: ArmsVsBody::Over,
                elbow: OverParent,
                wrist: OverParent,
                knee: UnderParent,
                ankle: UnderParent,
                head_over_body: true,
                mid_cloth_over_thighs: true,
                low_torso_over_body: false,
                pallu_over_body: false,
                right_leg_over_left: true,
            },
            ViewStacking {
                view: "side".into(),
                samples: 6,
                arms: ArmsVsBody::NearOverFarUnder,
                elbow: OverParent,
                wrist: UnderParent,
                knee: UnderParent,
                ankle: UnderParent,
                head_over_body: true,
                mid_cloth_over_thighs: true,
                low_torso_over_body: false,
                pallu_over_body: false,
                right_leg_over_left: true,
            },
            ViewStacking {
                view: "back".into(),
                samples: 6,
                arms: ArmsVsBody::Under,
                elbow: UnderParent,
                wrist: UnderParent,
                knee: UnderParent,
                ankle: UnderParent,
                head_over_body: true,
                mid_cloth_over_thighs: true,
                low_torso_over_body: false,
                pallu_over_body: true,
                right_leg_over_left: true,
            },
            ViewStacking {
                view: "back34".into(),
                samples: 3,
                arms: ArmsVsBody::Under,
                elbow: UnderParent,
                wrist: UnderParent,
                knee: UnderParent,
                ankle: UnderParent,
                head_over_body: true,
                mid_cloth_over_thighs: true,
                low_torso_over_body: false,
                pallu_over_body: false,
                right_leg_over_left: false,
            },
        ]
    }

    /// The measured stack for a view (`"front"`, `"side"`, `"back"`,
    /// `"back34"`), if the style has one.
    pub fn view(&self, view: &str) -> Option<&ViewStacking> {
        self.views.iter().find(|v| v.view == view)
    }

    /// The cut for a part key (`"Elbow"`), if the style has one.
    pub fn slot(&self, part: &str) -> Option<&SlotCut> {
        self.slots.iter().find(|s| s.part == part)
    }
}
