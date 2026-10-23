//! **How a head symbol is built**, as data: its layers top to bottom, which
//! layer is the eye mask and what it clips, which parts are frame-bearing
//! symbols (a blink, mouth shapes, eye shapes, two eyeballs), which frame
//! each part rests on, what the base holds and where the pivots sit.
//!
//! Beside [`crate::cut_style::CutStyle`] (how the body is cut): this one says
//! how the face inside the head piece is rigged. Measured, not designed:
//! [`HeadStyle::animate_convention`] holds what the user's own heads do, from
//! `docs/HEAD_RIG_PLAN.md` § H0 findings (21 heads in sampleNaming.fla,
//! morecharacters.fla and womanSample.fla).
//!
//! # Units
//!
//! * Pivots are fractions of the head's drawn bounds: `(0, 0)` its top-left,
//!   `(1, 1)` its bottom-right.
//! * Frames are 0-based (Animate's frame 1 is frame 0 here).
//! * Areas are fractions of the eye white's area.

use serde::{Deserialize, Serialize};

/// What a head layer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HeadPart {
    /// The blink symbol, over the eyes.
    EyeLid,
    /// The mask layer that clips the eyeballs.
    EyeMask,
    /// The pupils/irises, masked by the eye mask.
    EyeBall,
    /// The eye whites with their lash outline.
    EyeWhite,
    RightBrow,
    LeftBrow,
    Moustache,
    Mouth,
    Nose,
    /// Hair, turban or cap as its own layer (when it is not in the base).
    Hair,
    /// Skin, face shape and ears (and the hair, when there is no hair layer).
    #[default]
    Base,
}

/// How a layer takes part in masking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HeadLayerKind {
    #[default]
    Normal,
    /// Animate mask layer.
    Mask,
    /// Clipped by the mask layer above it.
    Masked,
}

/// How the part's symbol instance plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PartPlay {
    /// One frame, picked by its first frame (eye shapes, mouth shapes).
    #[default]
    SingleFrame,
    /// Plays from its first frame once, then holds (the blink).
    PlayOnce,
    Loop,
}

/// One layer of a head symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeadLayer {
    pub part: HeadPart,
    /// The user's layer name.
    pub name: String,
    pub kind: HeadLayerKind,
    /// Frames of the symbol the layer holds (1 = a plain drawing symbol).
    pub frames: u32,
    pub play: PartPlay,
    /// The frame it rests on.
    pub rest_frame: u32,
    /// Both eyes (or both brows) in one symbol on one layer.
    pub both_sides: bool,
    /// Every head has it (false: only some do, a moustache, a nose layer).
    pub always: bool,
    /// The transformation point, as a fraction of the head's bounds.
    pub pivot: (f64, f64),
}

impl Default for HeadLayer {
    fn default() -> Self {
        Self {
            part: HeadPart::Base,
            name: String::new(),
            kind: HeadLayerKind::Normal,
            frames: 1,
            play: PartPlay::SingleFrame,
            rest_frame: 0,
            both_sides: false,
            always: true,
            pivot: (0.5, 0.5),
        }
    }
}

/// The eyes: white, mask and eyeballs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EyeRig {
    /// The mask is the eye white's fill without its lash outline: the mask
    /// lies wholly inside the white (`mask_inside_white`) and covers this
    /// much of it.
    pub mask_from_white_fill: bool,
    /// Intersection over union of the mask and the white (with its outline).
    pub mask_vs_white_iou: f64,
    /// Share of the mask inside the white.
    pub mask_inside_white: f64,
    /// Eye shapes in the white symbol (normal, wide, narrow, squint).
    pub white_frames: u32,
    /// The mask symbol has the same frames as the white, frame for frame,
    /// and both rest on the same frame.
    pub mask_frames_match_white: bool,
    /// The frame the white and mask rest on.
    pub rest_shape_frame: u32,
    /// Frames of the eyeball symbol: a normal iris and small, frightened
    /// pupils.
    pub eyeball_frames: u32,
    /// Eyeball area over the white's, at rest.
    pub eyeball_area: f64,
    /// The second eyeball frame's area over the first's (front views).
    pub eyeball_second_frame_scale: f64,
    /// Gaze is the eyeball instance moved inside the mask, not a frame.
    pub gaze_by_moving_eyeball: bool,
    /// White, mask, eyeballs and lid share one pivot, between the eyes.
    pub eye_group_shares_pivot: bool,
}

impl Default for EyeRig {
    fn default() -> Self {
        Self {
            mask_from_white_fill: true,
            mask_vs_white_iou: 0.77,
            mask_inside_white: 1.0,
            white_frames: 4,
            mask_frames_match_white: true,
            rest_shape_frame: 1,
            eyeball_frames: 2,
            eyeball_area: 0.30,
            eyeball_second_frame_scale: 0.46,
            gaze_by_moving_eyeball: true,
            eye_group_shares_pivot: true,
        }
    }
}

/// The eyelid: a blink symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LidRig {
    /// Frames of the blink: open, closing, closed, opening, open.
    pub frames: u32,
    /// The fully closed frame.
    pub closed_frame: u32,
    /// Share of the eye white the lid covers, per frame.
    pub cover_per_frame: Vec<f64>,
    /// Frame 0 is the open eye: (almost) empty, a speck under 1 % of the
    /// white.
    pub open_frame_empty: bool,
    /// The lid is skin-coloured with a lash line along its lower edge.
    pub skin_with_lash_line: bool,
    /// The lid layer often holds a wrapper symbol (57-133 frames) that
    /// places the blink symbol "play once" at the frames the blinks happen.
    pub timing_wrapper: bool,
}

impl Default for LidRig {
    fn default() -> Self {
        Self {
            frames: 7,
            closed_frame: 3,
            cover_per_frame: vec![0.0, 0.50, 0.73, 1.0, 0.79, 0.53, 0.0],
            open_frame_empty: true,
            skin_with_lash_line: true,
            timing_wrapper: true,
        }
    }
}

/// **A way of building a head symbol**: its layers per view and the rules of
/// its eyes, lid, mouth and base.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeadStyle {
    pub name: String,
    /// Front heads, top layer first.
    pub front: Vec<HeadLayer>,
    /// Side (three-quarter) heads, top layer first.
    pub side: Vec<HeadLayer>,
    /// Back and three-quarter-back heads are one drawing: the hair (or
    /// turban), the ears and the nape. No face layers.
    pub back_single_drawing: bool,
    pub eyes: EyeRig,
    pub lid: LidRig,
    /// Mouth shapes in a front head's mouth symbol.
    pub mouth_frames_front: u32,
    /// Mouth shapes in a side head's mouth symbol.
    pub mouth_frames_side: u32,
    /// What the base holds.
    pub base_holds: Vec<String>,
    /// The nose is drawn in the base when the head has no nose layer.
    pub nose_in_base_without_layer: bool,
    /// Layer parenting inside a head: none.
    pub parenting: bool,
}

impl Default for HeadStyle {
    fn default() -> Self {
        Self::animate_convention()
    }
}

impl HeadStyle {
    /// **"My Animate head style"**: what the user's heads do (pradhanOldMan,
    /// man1Frontt, man2, mazdoor, newman1 and the woman; front, side and
    /// back). Layer order is pradhanOldMan's FrontHead, the order the plan
    /// names; brows and mouth sit above the lid in half the heads, which
    /// draws the same because they do not overlap the eyes.
    pub fn animate_convention() -> Self {
        let l = |part, name: &str, kind, frames, play, rest_frame, both_sides, always, pivot| HeadLayer {
            part,
            name: name.into(),
            kind,
            frames,
            play,
            rest_frame,
            both_sides,
            always,
            pivot,
        };
        use HeadLayerKind::{Mask, Masked, Normal};
        use HeadPart::*;
        use PartPlay::{PlayOnce, SingleFrame};
        let front = vec![
            // 5 of 6 front heads start the blink "play once" at the closed
            // frame (3), so the eyes open over frames 3-6 and hold open.
            l(EyeLid, "eyeLid", Normal, 7, PlayOnce, 3, true, true, (0.50, 0.51)),
            l(EyeMask, "eyeMask", Mask, 4, SingleFrame, 1, true, true, (0.50, 0.51)),
            l(EyeBall, "eyeBall", Masked, 2, SingleFrame, 0, true, true, (0.50, 0.52)),
            l(EyeWhite, "eyeWhite", Normal, 4, SingleFrame, 1, true, true, (0.50, 0.51)),
            l(RightBrow, "rightEyebrow", Normal, 1, SingleFrame, 0, false, true, (0.70, 0.43)),
            l(LeftBrow, "leftEyebrow", Normal, 1, SingleFrame, 0, false, true, (0.27, 0.44)),
            l(Moustache, "moustacheFront", Normal, 1, SingleFrame, 0, false, false, (0.53, 0.73)),
            l(Mouth, "mouth", Normal, 10, SingleFrame, 3, false, true, (0.51, 0.79)),
            l(Nose, "Nose", Normal, 1, SingleFrame, 0, false, false, (0.52, 0.61)),
            l(Hair, "hair", Normal, 1, SingleFrame, 0, false, false, (0.51, 0.33)),
            l(Base, "base", Normal, 1, SingleFrame, 0, false, true, (0.50, 0.50)),
        ];
        let side = vec![
            l(RightBrow, "rightEyebrow", Normal, 1, SingleFrame, 0, false, true, (0.58, 0.48)),
            l(LeftBrow, "leftEyebrow", Normal, 1, SingleFrame, 0, false, true, (0.34, 0.47)),
            l(EyeLid, "eyeLid", Normal, 7, PlayOnce, 0, true, true, (0.30, 0.50)),
            l(EyeMask, "eyeMask", Mask, 4, SingleFrame, 1, true, true, (0.36, 0.52)),
            l(EyeBall, "eyeBalls", Masked, 2, SingleFrame, 0, true, true, (0.33, 0.52)),
            l(EyeWhite, "eyeWhite", Normal, 4, SingleFrame, 1, true, true, (0.36, 0.52)),
            l(Nose, "nose", Normal, 1, SingleFrame, 0, false, false, (0.46, 0.63)),
            l(Moustache, "moustache", Normal, 1, SingleFrame, 0, false, false, (0.24, 0.75)),
            l(Mouth, "mouth", Normal, 14, SingleFrame, 12, false, true, (0.27, 0.80)),
            l(Hair, "hair", Normal, 1, SingleFrame, 0, false, false, (0.50, 0.28)),
            l(Base, "base", Normal, 1, SingleFrame, 0, false, true, (0.50, 0.50)),
        ];
        Self {
            name: "My Animate head style".into(),
            front,
            side,
            back_single_drawing: true,
            eyes: EyeRig::default(),
            lid: LidRig::default(),
            mouth_frames_front: 10,
            mouth_frames_side: 14,
            base_holds: [
                "skin face shape with its outline",
                "ears",
                "eye-socket shading and cheek blush",
                "wrinkles and chin line",
                "the hair, when the head has no hair layer (4 of 6 front heads, 5 of 6 side heads)",
                "the nose, when the head has no nose layer",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            nose_in_base_without_layer: true,
            parenting: false,
        }
    }

    /// The front layer for a part, if the style has one.
    pub fn front_layer(&self, part: HeadPart) -> Option<&HeadLayer> {
        self.front.iter().find(|l| l.part == part)
    }
}
