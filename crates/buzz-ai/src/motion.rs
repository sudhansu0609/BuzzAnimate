//! Poses and motions: standard pose library, pose-from-image via DWPose skeleton,
//! and motion classifier for keypoint sequences (walk vs run vs idle, hand gestures, eye movement).
//!
//! Chunk C7 specifications:
//! - Standard pose library on the rig: idle, walk, run, wave, point, sit via `perform::pose_at` and `apply`.
//! - Pose from image: DWPose on a reference image gives bone angles (`atan2` per bone relative to parent).
//! - Motion classifier: recognises motion in keypoint sequences (periodicity, flight phase, wrist speed, pupil offset).

use serde::{Deserialize, Serialize};

use buzz_act::figure::Joint;
use buzz_act::perform::{pose_at, Action, Beat, Performance};
use buzz_rig::{wrap_pi, Armature};
use buzz_scene::{ArmatureData, ObjectId, Scene};

use crate::character::Skeleton;

/// A named character pose consisting of joint angles for the 11 humanoid bones
/// and an optional root translation offset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterPose {
    pub name: String,
    pub description: String,
    /// 11 per-bone angle **deltas, added to the target armature's own rest
    /// pose** — not absolute angles. In `Joint::ALL` / `RigPattern::biped()`
    /// order. A traced character's rest angles come from its own drawing
    /// (`buzz_act::autorig::assemble`) and differ from the generic biped
    /// pattern's, so storing an absolute angle here would snap a bone to
    /// wherever the *generic* rig would put it rather than bending the
    /// character's own joint. See `apply_to_armature` and `to_named_poses`.
    pub angles: Vec<f64>,
    /// Root translation offset in document units (dx, dy).
    pub offset: (f64, f64),
}

impl CharacterPose {
    pub fn new(name: impl Into<String>, description: impl Into<String>, angles: Vec<f64>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            angles,
            offset: (0.0, 0.0),
        }
    }

    pub fn with_offset(mut self, offset: (f64, f64)) -> Self {
        self.offset = offset;
        self
    }
}

/// Standard pose library available to characters and Cast assets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoseLibrary {
    pub poses: Vec<CharacterPose>,
}

impl Default for PoseLibrary {
    fn default() -> Self {
        Self::standard()
    }
}

impl PoseLibrary {
    /// Generates the standard library of poses: idle, walk, run, wave, point, sit.
    ///
    /// Every pose's `angles` are deltas from rest — `Beat::joints` already are
    /// (see `buzz_act::perform`), so they are carried across unchanged, not
    /// added to a generic rig's rest angles the way this used to work. Applying
    /// a pose therefore always means rest-of-*this*-character plus delta; see
    /// `apply_to_armature`.
    pub fn standard() -> Self {
        // Beat.joints are already deltas added to rest; keep them as such.
        let beat_to_deltas = |beat: Beat| -> Vec<f64> {
            (0..11)
                .map(|i| beat.joints.get(i).copied().unwrap_or(0.0))
                .collect()
        };

        // 1. Idle (standing and breathing)
        let idle_beat = pose_at(Action::Idle, 0.0, 1.0);
        let idle = CharacterPose::new(
            "idle",
            "Standing still in neutral rest pose",
            beat_to_deltas(idle_beat),
        );

        // 2. Walk (contact phase of stride)
        let walk_beat = pose_at(Action::Walk, 0.25, 1.0);
        let walk_offset = (walk_beat.offset.x, walk_beat.offset.y);
        let walk = CharacterPose::new(
            "walk",
            "Mid-stride walk contact pose",
            beat_to_deltas(walk_beat),
        )
        .with_offset(walk_offset);

        // 3. Run (reach phase of running stride)
        let run_beat = pose_at(Action::Run, 0.25, 1.0);
        let run_offset = (run_beat.offset.x, run_beat.offset.y);
        let run = CharacterPose::new(
            "run",
            "Dynamic running stride with longer reach and bent arms",
            beat_to_deltas(run_beat),
        )
        .with_offset(run_offset);

        // 4. Wave (friendly raised arm greeting): raise the right shoulder and
        // bend the right elbow *relative to however this character's arm
        // actually hangs at rest*, rather than an absolute angle borrowed from
        // the generic biped pattern.
        let mut wave_deltas = vec![0.0; 11];
        wave_deltas[Joint::ShoulderR.index()] = 1.30;
        wave_deltas[Joint::ElbowR.index()] = 1.24;
        let wave = CharacterPose::new(
            "wave",
            "Friendly greeting with right hand raised and waved",
            wave_deltas,
        );

        // 5. Point (arm extended forward pointing)
        let point_beat = pose_at(Action::Point, 0.5, 1.0);
        let point = CharacterPose::new(
            "point",
            "Right arm extended forward pointing at target",
            beat_to_deltas(point_beat),
        );

        // 6. Sit (body lowered onto seat with bent knees)
        let sit_beat = pose_at(Action::Sit, 1.0, 1.0);
        let sit_offset = (sit_beat.offset.x, sit_beat.offset.y);
        let sit = CharacterPose::new(
            "sit",
            "Seated pose with folded knees and forward torso settle",
            beat_to_deltas(sit_beat),
        )
        .with_offset(sit_offset);

        Self {
            poses: vec![idle, walk, run, wave, point, sit],
        }
    }

    /// Finds a pose by name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&CharacterPose> {
        let n = name.to_lowercase();
        self.poses.iter().find(|p| p.name.to_lowercase() == n)
    }

    /// Applies a named pose to an ArmatureData.
    ///
    /// The pose's `angles` are deltas from rest, so this poses `armature`
    /// relative to *its own* rest pose (`armature.at_rest().pose()`) — the
    /// shape it was actually drawn in — rather than snapping every bone to an
    /// absolute angle borrowed from a generic rig.
    pub fn apply_to_armature(&self, pose_name: &str, armature: &mut ArmatureData) -> bool {
        if let Some(pose) = self.get(pose_name) {
            let deltas = resolve_deltas(pose, &armature.armature);
            let rest = armature.armature.at_rest().pose();
            let posed: Vec<f64> = rest
                .iter()
                .enumerate()
                .map(|(i, r)| wrap_pi(r + deltas.get(i).copied().unwrap_or(0.0)))
                .collect();
            armature.armature.set_pose(&posed);
            true
        } else {
            false
        }
    }

    /// Converts the library's poses to `NamedPose` instances for storing on
    /// `ArmatureData::poses`.
    ///
    /// `NamedPose::angles` are absolute per-bone pose angles (the shape
    /// `Armature::pose` returns), so each of this library's rest-relative
    /// deltas is resolved against `target`'s *own* rest pose here — the rig
    /// the poses will actually be stored on and applied to.
    pub fn to_named_poses(&self, target: &ArmatureData) -> Vec<buzz_scene::NamedPose> {
        let rest = target.armature.at_rest().pose();
        self.poses
            .iter()
            .map(|p| {
                let deltas = resolve_deltas(p, &target.armature);
                buzz_scene::NamedPose {
                    name: p.name.clone(),
                    angles: rest
                        .iter()
                        .enumerate()
                        .map(|(i, r)| wrap_pi(r + deltas.get(i).copied().unwrap_or(0.0)))
                        .collect(),
                }
            })
            .collect()
    }

    /// Writes keyframes for a standard perform action onto a scene object timeline.
    pub fn apply_action(
        scene: &mut Scene,
        object_id: ObjectId,
        action: Action,
        frames: std::ops::Range<u32>,
        amount: f64,
    ) -> Result<buzz_act::perform::PerformReport, buzz_act::perform::PerformError> {
        let performance = Performance {
            action,
            frames,
            amount,
            tempo: 1.0,
            distance: if action.travels() { 120.0 } else { 0.0 },
            step: 2,
        };
        buzz_act::perform::apply(scene, object_id, &performance)
    }
}

/// Where the "wave" pose wants the forearm pointing, in world space (y
/// downward, so `-FRAC_PI_2` is straight up).
const WAVE_FOREARM_WORLD_ANGLE: f64 = -std::f64::consts::FRAC_PI_2;

/// Resolves a pose's per-bone deltas against a specific armature.
///
/// Every pose but "wave" is unchanged. "Wave"'s `ElbowR` delta is a fixed
/// number chosen for a generic rig whose arm hangs straight down at rest; on
/// a character whose arm is already bent at rest (a namaste-like drawn
/// pose), the same fixed delta lands the forearm pointing sideways or down
/// instead of up. So it is recomputed here, from *this* armature's own rest
/// pose, so the forearm's *world* angle ends up pointing up
/// (`WAVE_FOREARM_WORLD_ANGLE`) regardless of how the arm rests: world angle
/// is the sum of local angles up the chain, and `ShoulderR`'s delta is part
/// of that sum, so `ElbowR`'s delta is the target minus the shoulder's
/// contribution minus the elbow's own rest angle.
fn resolve_deltas(pose: &CharacterPose, armature: &Armature) -> Vec<f64> {
    let mut deltas = pose.angles.clone();
    if pose.name == "wave" {
        let elbow_idx = Joint::ElbowR.index();
        let shoulder_idx = Joint::ShoulderR.index();
        if elbow_idx < deltas.len() {
            let shoulder_delta = deltas.get(shoulder_idx).copied().unwrap_or(0.0);
            let rest_world_elbow = armature.at_rest().world_angle(elbow_idx);
            deltas[elbow_idx] = wrap_pi(WAVE_FOREARM_WORLD_ANGLE - shoulder_delta - rest_world_elbow);
        }
    }
    deltas
}

/// Computes the angle of vector (p1 -> p2) with y pointing downward: `atan2(y2 - y1, x2 - x1)`.
fn vector_angle(p1: (f64, f64), p2: (f64, f64)) -> f64 {
    (p2.1 - p1.1).atan2(p2.0 - p1.0)
}

/// Estimates bone angles from a detected `Skeleton` using `atan2` relative to parent.
///
/// Returns a `CharacterPose` containing the 11 humanoid bone angles. **These
/// are absolute pose angles** (the shape `Armature::pose` / `Bone::angle`
/// use), not deltas from rest — they are measured directly off the detected
/// skeleton's own joint positions, so there is no separate rest pose to take
/// a delta against. Unlike `PoseLibrary`'s poses, do not add these to a
/// target armature's rest pose before calling `set_pose`.
pub fn pose_from_skeleton(skel: &Skeleton, pose_name: &str) -> CharacterPose {
    let get_joint = |name: &str| -> Option<(f64, f64)> {
        skel.joint(name).map(|j| (j.x as f64, j.y as f64))
    };

    let pelvis = get_joint("Hips").unwrap_or((250.0, 300.0));
    let chest = get_joint("Chest").unwrap_or((250.0, 220.0));
    let neck = get_joint("Neck").unwrap_or((250.0, 180.0));
    let head = get_joint("Head").unwrap_or((250.0, 140.0));

    let sh_l = get_joint("Shoulder L").unwrap_or((220.0, 200.0));
    let el_l = get_joint("Elbow L").unwrap_or((200.0, 270.0));
    let wr_l = get_joint("Hand L").or_else(|| get_joint("Wrist L")).unwrap_or((190.0, 340.0));

    let sh_r = get_joint("Shoulder R").unwrap_or((280.0, 200.0));
    let el_r = get_joint("Elbow R").unwrap_or((300.0, 270.0));
    let wr_r = get_joint("Hand R").or_else(|| get_joint("Wrist R")).unwrap_or((310.0, 340.0));

    let hip_l = get_joint("Thigh L").unwrap_or((230.0, 320.0));
    let kn_l = get_joint("Knee L").unwrap_or((230.0, 420.0));
    let foot_l = get_joint("Foot L").or_else(|| get_joint("Ankle L")).unwrap_or((230.0, 520.0));

    let hip_r = get_joint("Thigh R").unwrap_or((270.0, 320.0));
    let kn_r = get_joint("Knee R").unwrap_or((270.0, 420.0));
    let foot_r = get_joint("Foot R").or_else(|| get_joint("Ankle R")).unwrap_or((270.0, 520.0));

    // World bone directions
    let w_hips = vector_angle(pelvis, chest);
    let w_chest = vector_angle(chest, neck);
    let w_head = vector_angle(neck, head);

    let w_sh_l = vector_angle(sh_l, el_l);
    let w_el_l = vector_angle(el_l, wr_l);

    let w_sh_r = vector_angle(sh_r, el_r);
    let w_el_r = vector_angle(el_r, wr_r);

    let w_thigh_l = vector_angle(hip_l, kn_l);
    let w_knee_l = vector_angle(kn_l, foot_l);

    let w_thigh_r = vector_angle(hip_r, kn_r);
    let w_knee_r = vector_angle(kn_r, foot_r);

    // Hierarchy according to RigPattern::biped():
    // 0: Hips (parent: None) -> w_hips
    // 1: Chest (parent: Hips) -> w_chest - w_hips
    // 2: Head (parent: Chest) -> w_head - w_chest
    // 3: Shoulder L (parent: Chest) -> w_sh_l - w_chest
    // 4: Elbow L (parent: Shoulder L) -> w_el_l - w_sh_l
    // 5: Shoulder R (parent: Chest) -> w_sh_r - w_chest
    // 6: Elbow R (parent: Shoulder R) -> w_el_r - w_sh_r
    // 7: Thigh L (parent: None) -> w_thigh_l
    // 8: Knee L (parent: Thigh L) -> w_knee_l - w_thigh_l
    // 9: Thigh R (parent: None) -> w_thigh_r
    // 10: Knee R (parent: Thigh R) -> w_knee_r - w_thigh_r

    let angles = vec![
        wrap_pi(w_hips),
        wrap_pi(w_chest - w_hips),
        wrap_pi(w_head - w_chest),
        wrap_pi(w_sh_l - w_chest),
        wrap_pi(w_el_l - w_sh_l),
        wrap_pi(w_sh_r - w_chest),
        wrap_pi(w_el_r - w_sh_r),
        wrap_pi(w_thigh_l),
        wrap_pi(w_knee_l - w_thigh_l),
        wrap_pi(w_thigh_r),
        wrap_pi(w_knee_r - w_thigh_r),
    ];

    CharacterPose::new(
        pose_name,
        format!("Pose estimated from skeleton (height {})", skel.character_height),
        angles,
    )
}

/// Single-frame sample of keypoints for motion classification.
#[derive(Debug, Clone)]
pub struct KeypointFrame {
    pub frame: u32,
    /// 133 DWPose keypoints (x, y, conf).
    pub keypoints: [(f32, f32, f32); 133],
    /// Optional pupil gaze offset (-1.0..=1.0)
    pub pupil_offset: Option<(f64, f64)>,
}

impl KeypointFrame {
    pub fn from_keypoints(frame: u32, keypoints: [(f32, f32, f32); 133]) -> Self {
        Self {
            frame,
            keypoints,
            pupil_offset: None,
        }
    }

    pub fn with_pupil(mut self, pupil: (f64, f64)) -> Self {
        self.pupil_offset = Some(pupil);
        self
    }
}

/// A sequence of keypoint frames recorded over time.
#[derive(Debug, Clone)]
pub struct MotionSequence {
    pub frames: Vec<KeypointFrame>,
    pub fps: f64,
}

impl MotionSequence {
    pub fn new(fps: f64) -> Self {
        Self {
            frames: Vec::new(),
            fps: fps.max(1.0),
        }
    }

    pub fn push(&mut self, frame: KeypointFrame) {
        self.frames.push(frame);
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Locomotion category classified from keypoint trajectory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocomotionClassification {
    Idle,
    Walk,
    Run,
}

/// Outcome of motion analysis across a keypoint sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotionClassification {
    pub locomotion: LocomotionClassification,
    /// Estimated frequency of strides (cycles per second).
    pub strides_per_second: f64,
    /// Whether both feet were off the ground simultaneously during stride.
    pub has_flight_phase: bool,
    /// Detected active hand gesture (wave, point, reach).
    pub hand_gesture: bool,
    /// Detected eye movement / gaze changes.
    pub eye_movement: bool,
    /// Confidence score of the classification (0.0..=1.0).
    pub confidence: f64,
}

/// Autocorrelation helper for periodic trajectory detection.
fn autocorrelation(series: &[f64], max_lag: usize) -> Vec<f64> {
    let n = series.len();
    if n == 0 {
        return Vec::new();
    }
    let mean = series.iter().sum::<f64>() / n as f64;
    let var = series.iter().map(|&x| (x - mean).powi(2)).sum::<f64>();
    if var.abs() < 1e-6 {
        return vec![0.0; max_lag + 1];
    }

    let mut ac = Vec::with_capacity(max_lag + 1);
    for lag in 0..=max_lag {
        let mut cov = 0.0;
        for i in 0..(n - lag) {
            cov += (series[i] - mean) * (series[i + lag] - mean);
        }
        ac.push(cov / var);
    }
    ac
}

/// Classifies motion in a sequence of keypoints according to Chunk C7 rules:
/// 1. Ankle-trajectory periodicity (autocorrelation) and stride per second.
/// 2. Flight phase: both feet off the ground marks a Run.
/// 3. Wrist speed or amplitude above threshold marks a Hand Gesture.
/// 4. Pupil offset changes mark Eye Movement.
pub fn classify_motion(sequence: &MotionSequence) -> MotionClassification {
    let n = sequence.frames.len();
    if n < 4 {
        return MotionClassification {
            locomotion: LocomotionClassification::Idle,
            strides_per_second: 0.0,
            has_flight_phase: false,
            hand_gesture: false,
            eye_movement: false,
            confidence: 0.5,
        };
    }

    // Keypoint indices in COCO-WholeBody (133 points):
    // Left Ankle: 15, Right Ankle: 16
    // Left Wrist: 9, Right Wrist: 10
    // Nose / Head: 0
    let mut ank_l_x = Vec::with_capacity(n);
    let mut ank_l_y = Vec::with_capacity(n);
    let mut ank_r_x = Vec::with_capacity(n);
    let mut ank_r_y = Vec::with_capacity(n);
    let mut wrist_l_y = Vec::with_capacity(n);
    let mut wrist_r_y = Vec::with_capacity(n);
    let mut wrist_speeds = Vec::with_capacity(n);
    let mut pupil_changes = 0.0f64;

    let mut approx_height = 100.0f64;

    for (i, f) in sequence.frames.iter().enumerate() {
        let al = f.keypoints[15];
        let ar = f.keypoints[16];
        let wl = f.keypoints[9];
        let wr = f.keypoints[10];
        let head = f.keypoints[0];

        let cur_h = ((al.1.max(ar.1)) - head.1).abs() as f64;
        if cur_h > 50.0 {
            approx_height = cur_h;
        }

        ank_l_x.push(al.0 as f64);
        ank_l_y.push(al.1 as f64);
        ank_r_x.push(ar.0 as f64);
        ank_r_y.push(ar.1 as f64);

        wrist_l_y.push(wl.1 as f64);
        wrist_r_y.push(wr.1 as f64);

        if i > 0 {
            let prev = &sequence.frames[i - 1];
            let prev_wl = prev.keypoints[9];
            let prev_wr = prev.keypoints[10];
            let spd_l = (((wl.0 - prev_wl.0).powi(2) + (wl.1 - prev_wl.1).powi(2)) as f64).sqrt();
            let spd_r = (((wr.0 - prev_wr.0).powi(2) + (wr.1 - prev_wr.1).powi(2)) as f64).sqrt();
            wrist_speeds.push(spd_l.max(spd_r));

            if let (Some(cur_p), Some(prev_p)) = (f.pupil_offset, prev.pupil_offset) {
                let d = ((cur_p.0 - prev_p.0).hypot(cur_p.1 - prev_p.1)).abs();
                pupil_changes += d;
            }
        }
    }

    // 1. Locomotion Periodicity & Amplitude
    // Ankle separation delta x(t)
    let ankle_sep: Vec<f64> = (0..n).map(|i| ank_l_x[i] - ank_r_x[i]).collect();
    let min_sep = ankle_sep.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_sep = ankle_sep.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let sep_range = max_sep - min_sep;

    let min_y_l = ank_l_y.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_y_l = ank_l_y.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let vert_range = max_y_l - min_y_l;

    let is_moving = sep_range > approx_height * 0.12 || vert_range > approx_height * 0.08;

    let (locomotion, strides_per_second, has_flight_phase) = if !is_moving {
        (LocomotionClassification::Idle, 0.0, false)
    } else {
        // Autocorrelation to detect stride frequency
        let max_lag = (n / 2).min(60);
        let ac = autocorrelation(&ankle_sep, max_lag);

        // Find first positive peak after initial decline
        let mut peak_lag = 0;
        let mut min_val = 1.0;
        let mut found_min = false;

        for (lag, &val) in ac.iter().enumerate().skip(2) {
            if !found_min {
                if val < min_val {
                    min_val = val;
                } else if val > min_val + 0.1 {
                    found_min = true;
                }
            } else if val > 0.1 {
                if lag + 1 < ac.len() && val >= ac[lag - 1] && val >= ac[lag + 1] {
                    peak_lag = lag;
                    break;
                }
            }
        }

        let stride_hz = if peak_lag > 0 {
            sequence.fps / peak_lag as f64
        } else {
            // Default 1.5 - 2.0 strides / sec
            1.5
        };

        // 2. Flight Phase Detection:
        // Ground baseline = highest y (deepest contact with ground)
        let ground_baseline = ank_l_y
            .iter()
            .chain(ank_r_y.iter())
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);

        // Check if both feet leave the ground simultaneously:
        // min(ground - y_L, ground - y_R) > threshold
        let mut flight_frames = 0;
        let flight_threshold = (approx_height * 0.05).max(4.0);

        for i in 0..n {
            let dist_l = ground_baseline - ank_l_y[i];
            let dist_r = ground_baseline - ank_r_y[i];
            if dist_l > flight_threshold && dist_r > flight_threshold {
                flight_frames += 1;
            }
        }

        let has_flight = flight_frames >= 2 || (stride_hz > 2.2);

        let kind = if has_flight {
            LocomotionClassification::Run
        } else {
            LocomotionClassification::Walk
        };

        (kind, stride_hz, has_flight)
    };

    // 3. Hand Gesture Detection:
    // Wrist moving faster than normal walk sway or held up high
    let avg_wrist_speed = if !wrist_speeds.is_empty() {
        wrist_speeds.iter().sum::<f64>() / wrist_speeds.len() as f64
    } else {
        0.0
    };
    let max_wrist_speed = wrist_speeds.iter().cloned().fold(0.0f64, f64::max);
    let hand_gesture = max_wrist_speed > approx_height * 0.08 || avg_wrist_speed > approx_height * 0.035;

    // 4. Eye Movement Detection:
    let eye_movement = pupil_changes > 0.25;

    MotionClassification {
        locomotion,
        strides_per_second,
        has_flight_phase,
        hand_gesture,
        eye_movement,
        confidence: 0.90,
    }
}
