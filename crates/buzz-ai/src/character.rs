//! Character skeleton, joint mapping, and part structuring.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use crate::pose::Person;
use crate::segment::{self, Sam2Point};
use buzz_geom::{Affine, BezPath, BoolOp, BooleanOptions, Point, Rect, Shape, Size};
use buzz_rig::RigPattern;
use buzz_scene::{
    ArmatureData, Blend, Layer, LayerId, LayerKind, Object, ObjectId, ObjectKind, Scene, Symbol,
    SymbolId, SymbolKind, Turnaround,
};
use kurbo::flatten;

/// A named joint in the character skeleton.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Joint {
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub confidence: f32,
}

/// A bone connecting two joints or defining a limb segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    pub head: (f32, f32),
    pub tail: (f32, f32),
    pub confidence: f32,
    pub length: f32,
}

/// Key facial feature point groups extracted from whole-body keypoints.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FaceFeatures {
    /// Left eye contour points (COCO-WholeBody 59..65).
    pub left_eye: Vec<(f32, f32, f32)>,
    /// Right eye contour points (COCO-WholeBody 65..71).
    pub right_eye: Vec<(f32, f32, f32)>,
    /// Left eyebrow points (COCO-WholeBody 40..45).
    pub left_brow: Vec<(f32, f32, f32)>,
    /// Right eyebrow points (COCO-WholeBody 45..50).
    pub right_brow: Vec<(f32, f32, f32)>,
    /// Mouth contour points (COCO-WholeBody 71..91).
    pub mouth: Vec<(f32, f32, f32)>,
}

/// Full character skeleton mapped to humanoid rig slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
    pub bones: Vec<Bone>,
    pub face: FaceFeatures,
    pub character_height: f32,
}

impl Skeleton {
    /// Finds a joint by name.
    pub fn joint(&self, name: &str) -> Option<&Joint> {
        self.joints.iter().find(|j| j.name.eq_ignore_ascii_case(name))
    }

    /// Finds a bone by name.
    pub fn bone(&self, name: &str) -> Option<&Bone> {
        self.bones.iter().find(|b| b.name.eq_ignore_ascii_case(name))
    }
}

/// Maps COCO-WholeBody 133 keypoints from `Person` into humanoid `RigPattern` slots.
///
/// Slots covered:
/// - Head, Neck, Torso / Pelvis (Hips, Chest)
/// - Upper Arm L ("Shoulder L"), Forearm L ("Elbow L"), Hand L
/// - Upper Arm R ("Shoulder R"), Forearm R ("Elbow R"), Hand R
/// - Thigh L, Shin L ("Knee L"), Foot L
/// - Thigh R, Shin R ("Knee R"), Foot R
///
/// Also preserves facial features (eyes, brows, mouth) for facial animation.
pub fn skeleton(person: &Person) -> Skeleton {
    let kp = &person.keypoints;
    let height = person.character_height();

    // Helper to get keypoint (x, y, conf)
    let get_kp = |idx: usize| -> (f32, f32, f32) {
        if idx < kp.len() {
            kp[idx]
        } else {
            (0.0, 0.0, 0.0)
        }
    };

    let nose = get_kp(0);
    let eye_l = get_kp(1);
    let eye_r = get_kp(2);
    let ear_l = get_kp(3);
    let ear_r = get_kp(4);
    let sh_l = get_kp(5);
    let sh_r = get_kp(6);
    let el_l = get_kp(7);
    let el_r = get_kp(8);
    let wr_l = get_kp(9);
    let wr_r = get_kp(10);
    let hip_l = get_kp(11);
    let hip_r = get_kp(12);
    let kn_l = get_kp(13);
    let kn_r = get_kp(14);
    let ank_l = get_kp(15);
    let ank_r = get_kp(16);

    // Neck: midpoint of shoulders
    let neck_pt = (
        (sh_l.0 + sh_r.0) * 0.5,
        (sh_l.1 + sh_r.1) * 0.5,
        (sh_l.2.min(sh_r.2)),
    );

    // Hips / Pelvis: midpoint of hips
    let pelvis_pt = (
        (hip_l.0 + hip_r.0) * 0.5,
        (hip_l.1 + hip_r.1) * 0.5,
        (hip_l.2.min(hip_r.2)),
    );

    // Chest / Torso: midpoint of neck and pelvis
    let chest_pt = (
        (neck_pt.0 + pelvis_pt.0) * 0.5,
        (neck_pt.1 + pelvis_pt.1) * 0.5,
        (neck_pt.2.min(pelvis_pt.2)),
    );

    // Head top: estimate from nose, eyes, ears
    let head_center = if ear_l.2 > 0.2 && ear_r.2 > 0.2 {
        ((ear_l.0 + ear_r.0) * 0.5, (ear_l.1 + ear_r.1) * 0.5, ear_l.2.min(ear_r.2))
    } else if eye_l.2 > 0.2 && eye_r.2 > 0.2 {
        ((eye_l.0 + eye_r.0) * 0.5, (eye_l.1 + eye_r.1) * 0.5, eye_l.2.min(eye_r.2))
    } else {
        (nose.0, nose.1 - height * 0.05, nose.2)
    };
    let head_pt = (
        head_center.0,
        head_center.1 - (neck_pt.1 - head_center.1).abs().max(height * 0.08),
        head_center.2,
    );

    // Hands: calculate centroid of hand keypoints if confident, else extend wrist
    let calc_hand = |wr: (f32, f32, f32), el: (f32, f32, f32), start_idx: usize| -> (f32, f32, f32) {
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_c = 0.0;
        let mut count = 0;
        for i in start_idx..(start_idx + 21) {
            let p = get_kp(i);
            if p.2 > 0.2 {
                sum_x += p.0;
                sum_y += p.1;
                sum_c += p.2;
                count += 1;
            }
        }
        if count >= 3 {
            (sum_x / count as f32, sum_y / count as f32, sum_c / count as f32)
        } else {
            // Direction from elbow to wrist
            let dx = wr.0 - el.0;
            let dy = wr.1 - el.1;
            let d = (dx * dx + dy * dy).sqrt().max(1.0);
            let ext = height * 0.07;
            (wr.0 + dx / d * ext, wr.1 + dy / d * ext, wr.2 * 0.8)
        }
    };

    let hand_l_pt = calc_hand(wr_l, el_l, 91);
    let hand_r_pt = calc_hand(wr_r, el_r, 112);

    // Feet: calculate centroid of foot keypoints if confident, else extend ankle
    let calc_foot = |ank: (f32, f32, f32), kn: (f32, f32, f32), start_idx: usize| -> (f32, f32, f32) {
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_c = 0.0;
        let mut count = 0;
        for i in start_idx..(start_idx + 3) {
            let p = get_kp(i);
            if p.2 > 0.2 {
                sum_x += p.0;
                sum_y += p.1;
                sum_c += p.2;
                count += 1;
            }
        }
        if count >= 1 {
            (sum_x / count as f32, sum_y / count as f32, sum_c / count as f32)
        } else {
            // Extend ankle downwards
            let dx = ank.0 - kn.0;
            let dy = ank.1 - kn.1;
            let d = (dx * dx + dy * dy).sqrt().max(1.0);
            let ext = height * 0.06;
            (ank.0 + dx / d * ext, ank.1 + dy / d * ext, ank.2 * 0.8)
        }
    };

    let foot_l_pt = calc_foot(ank_l, kn_l, 17);
    let foot_r_pt = calc_foot(ank_r, kn_r, 20);

    // Build joints list
    let joints = vec![
        Joint { name: "Head".into(), x: head_pt.0, y: head_pt.1, confidence: head_pt.2 },
        Joint { name: "Nose".into(), x: nose.0, y: nose.1, confidence: nose.2 },
        Joint { name: "Neck".into(), x: neck_pt.0, y: neck_pt.1, confidence: neck_pt.2 },
        Joint { name: "Chest".into(), x: chest_pt.0, y: chest_pt.1, confidence: chest_pt.2 },
        Joint { name: "Hips".into(), x: pelvis_pt.0, y: pelvis_pt.1, confidence: pelvis_pt.2 },
        Joint { name: "Shoulder L".into(), x: sh_l.0, y: sh_l.1, confidence: sh_l.2 },
        Joint { name: "Elbow L".into(), x: el_l.0, y: el_l.1, confidence: el_l.2 },
        Joint { name: "Wrist L".into(), x: wr_l.0, y: wr_l.1, confidence: wr_l.2 },
        Joint { name: "Hand L".into(), x: hand_l_pt.0, y: hand_l_pt.1, confidence: hand_l_pt.2 },
        Joint { name: "Shoulder R".into(), x: sh_r.0, y: sh_r.1, confidence: sh_r.2 },
        Joint { name: "Elbow R".into(), x: el_r.0, y: el_r.1, confidence: el_r.2 },
        Joint { name: "Wrist R".into(), x: wr_r.0, y: wr_r.1, confidence: wr_r.2 },
        Joint { name: "Hand R".into(), x: hand_r_pt.0, y: hand_r_pt.1, confidence: hand_r_pt.2 },
        Joint { name: "Thigh L".into(), x: hip_l.0, y: hip_l.1, confidence: hip_l.2 },
        Joint { name: "Knee L".into(), x: kn_l.0, y: kn_l.1, confidence: kn_l.2 },
        Joint { name: "Ankle L".into(), x: ank_l.0, y: ank_l.1, confidence: ank_l.2 },
        Joint { name: "Foot L".into(), x: foot_l_pt.0, y: foot_l_pt.1, confidence: foot_l_pt.2 },
        Joint { name: "Thigh R".into(), x: hip_r.0, y: hip_r.1, confidence: hip_r.2 },
        Joint { name: "Knee R".into(), x: kn_r.0, y: kn_r.1, confidence: kn_r.2 },
        Joint { name: "Ankle R".into(), x: ank_r.0, y: ank_r.1, confidence: ank_r.2 },
        Joint { name: "Foot R".into(), x: foot_r_pt.0, y: foot_r_pt.1, confidence: foot_r_pt.2 },
    ];

    // Helper to build a bone
    let make_bone = |name: &str, h: (f32, f32, f32), t: (f32, f32, f32)| -> Bone {
        let dx = t.0 - h.0;
        let dy = t.1 - h.1;
        let length = (dx * dx + dy * dy).sqrt();
        let confidence = h.2.min(t.2);
        Bone {
            name: name.to_string(),
            head: (h.0, h.1),
            tail: (t.0, t.1),
            confidence,
            length,
        }
    };

    // Build humanoid bones
    let bones = vec![
        make_bone("Hips", pelvis_pt, chest_pt),
        make_bone("Chest", chest_pt, neck_pt),
        make_bone("Head", neck_pt, head_pt),
        make_bone("Shoulder L", sh_l, el_l),
        make_bone("Elbow L", el_l, wr_l),
        // T10: the hand and foot bones are the WRIST and ANKLE bones (wrist to hand
        // point, ankle to foot point), named for the `biped_full` slots they ride.
        make_bone("Wrist L", wr_l, hand_l_pt),
        make_bone("Shoulder R", sh_r, el_r),
        make_bone("Elbow R", el_r, wr_r),
        make_bone("Wrist R", wr_r, hand_r_pt),
        make_bone("Thigh L", hip_l, kn_l),
        make_bone("Knee L", kn_l, ank_l),
        make_bone("Ankle L", ank_l, foot_l_pt),
        make_bone("Thigh R", hip_r, kn_r),
        make_bone("Knee R", kn_r, ank_r),
        make_bone("Ankle R", ank_r, foot_r_pt),
    ];

    // Facial feature points
    let slice_kp = |start: usize, end: usize| -> Vec<(f32, f32, f32)> {
        (start..end.min(kp.len())).map(get_kp).collect()
    };

    let face = FaceFeatures {
        left_eye: slice_kp(59, 65),
        right_eye: slice_kp(65, 71),
        left_brow: slice_kp(40, 45),
        right_brow: slice_kp(45, 50),
        mouth: slice_kp(71, 91),
    };

    Skeleton {
        joints,
        bones,
        face,
        character_height: height,
    }
}

/// Whether the detected leg joints are unreliable enough to need the
/// silhouette-split fallback (LOCAL_AI_PLAN.md "Opus review of the quality
/// run", item 6): any hip/knee/ankle joint below 0.5 confidence, or every
/// corresponding L/R joint pair sitting within 10% of character height of
/// each other (DWPose couldn't tell the legs apart).
pub fn legs_need_fallback(skel: &Skeleton) -> bool {
    let height = skel.character_height.max(1.0);
    let low_conf = ["Thigh L", "Knee L", "Ankle L", "Thigh R", "Knee R", "Ankle R"]
        .iter()
        .any(|n| skel.joint(n).map(|j| j.confidence).unwrap_or(0.0) < 0.5);
    if low_conf {
        return true;
    }
    let pairs = [("Thigh L", "Thigh R"), ("Knee L", "Knee R"), ("Ankle L", "Ankle R")];
    pairs.iter().all(|(a, b)| match (skel.joint(a), skel.joint(b)) {
        (Some(ja), Some(jb)) => {
            let d = ((ja.x - jb.x).powi(2) + (ja.y - jb.y).powi(2)).sqrt();
            d < height * 0.10
        }
        _ => false,
    })
}

/// Extracts simple-path branches from a thinned skeleton: from every
/// endpoint (a skeleton pixel with exactly one skeleton neighbour), walks
/// until it hits a junction (3+ neighbours) or closes on another endpoint.
/// Each returned branch is ordered top-to-bottom (smallest y first).
fn extract_downward_branches(skel: &[u8], w: u32, h: u32) -> Vec<Vec<(i32, i32)>> {
    let (wi, hi) = (w as i32, h as i32);
    let is_set = |x: i32, y: i32| -> bool {
        x >= 0 && y >= 0 && x < wi && y < hi && skel[(y * wi + x) as usize] > 128
    };
    let neighbours = |x: i32, y: i32| -> Vec<(i32, i32)> {
        let mut v = Vec::new();
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                if is_set(x + dx, y + dy) {
                    v.push((x + dx, y + dy));
                }
            }
        }
        v
    };
    let mut endpoints = Vec::new();
    for y in 0..hi {
        for x in 0..wi {
            if is_set(x, y) && neighbours(x, y).len() == 1 {
                endpoints.push((x, y));
            }
        }
    }
    let mut branches = Vec::new();
    let mut used_edges: std::collections::HashSet<((i32, i32), (i32, i32))> = std::collections::HashSet::new();
    let max_len = (w as usize * h as usize) + 1;
    for &start in &endpoints {
        let mut path = vec![start];
        let mut prev = start;
        let mut cur = start;
        loop {
            let nbrs = neighbours(cur.0, cur.1);
            let next = nbrs
                .iter()
                .find(|&&p| p != prev && !used_edges.contains(&(cur, p)) && !used_edges.contains(&(p, cur)))
                .copied();
            let Some(next) = next else {
                break;
            };
            used_edges.insert((cur, next));
            path.push(next);
            if next == start || neighbours(next.0, next.1).len() >= 3 || path.len() > max_len {
                break;
            }
            prev = cur;
            cur = next;
        }
        if path.len() >= 2 {
            if path.first().unwrap().1 > path.last().unwrap().1 {
                path.reverse();
            }
            branches.push(path);
        }
    }
    branches
}

/// Fallback-of-fallback for a degenerate skeleton (no junction/endpoint
/// structure found): splits the crop into left/right halves by its vertical
/// midline and takes each half's own skeleton pixels, sorted top-to-bottom,
/// as that side's branch.
fn split_half_branches(binary: &[u8], w: u32, h: u32) -> Vec<Vec<(i32, i32)>> {
    let mid = w / 2;
    let mut out = Vec::new();
    for (x0, x1) in [(0, mid), (mid, w)] {
        if x1 <= x0 {
            continue;
        }
        let mut half = vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in x0..x1 {
                half[(y * w + x) as usize] = binary[(y * w + x) as usize];
            }
        }
        let sk = segment::zhang_suen_thin(&half, w, h);
        let mut pts: Vec<(i32, i32)> = Vec::new();
        for y in 0..h as i32 {
            for x in x0 as i32..x1 as i32 {
                if sk[(y as u32 * w + x as u32) as usize] > 128 {
                    pts.push((x, y));
                }
            }
        }
        if pts.len() >= 2 {
            pts.sort_by_key(|p| p.1);
            out.push(pts);
        }
    }
    out
}

/// Silhouette-split leg fallback (item 6). When [`legs_need_fallback`], crops
/// the matte below the hips, thins it to a 1px skeleton, and takes the two
/// longest downward branches as the legs (falling back to a left/right half
/// split when the skeleton has no clean branch structure), with the knee at
/// the branch's 50% arc-length point, ankle at 88%, foot tip at 100%.
/// Mutates `skel`'s leg joints and bones in place; returns whether the
/// fallback fired.
pub fn leg_fallback_silhouette_split(skel: &mut Skeleton, matte: &image::GrayImage) -> bool {
    if !legs_need_fallback(skel) {
        return false;
    }
    let Some(hips) = skel.joint("Hips").cloned() else {
        return false;
    };
    let (w, h) = (matte.width(), matte.height());
    if h == 0 || w == 0 {
        return false;
    }
    let top = (hips.y.round() as i32).clamp(0, h as i32 - 2) as u32;
    let crop_h = h - top;

    let mut binary = vec![0u8; (w * crop_h) as usize];
    for y in 0..crop_h {
        for x in 0..w {
            binary[(y * w + x) as usize] = if matte.get_pixel(x, top + y)[0] > 128 { 255 } else { 0 };
        }
    }
    let skeleton_px = segment::zhang_suen_thin(&binary, w, crop_h);

    let mut branches = extract_downward_branches(&skeleton_px, w, crop_h);
    if branches.len() < 2 {
        branches = split_half_branches(&binary, w, crop_h);
    }
    if branches.len() < 2 {
        return false;
    }
    branches.sort_by_key(|b| std::cmp::Reverse(b.len()));
    let mut top2: Vec<Vec<(i32, i32)>> = branches.into_iter().take(2).collect();
    let mean_x = |b: &[(i32, i32)]| -> f64 { b.iter().map(|p| p.0 as f64).sum::<f64>() / (b.len().max(1) as f64) };
    if mean_x(&top2[0]) > mean_x(&top2[1]) {
        top2.swap(0, 1);
    }

    let make_bone = |name: &str, head: (f32, f32), tail: (f32, f32)| -> Bone {
        let dx = tail.0 - head.0;
        let dy = tail.1 - head.1;
        Bone { name: name.to_string(), head, tail, confidence: 0.5, length: (dx * dx + dy * dy).sqrt() }
    };

    for (side, branch) in [("L", &top2[0]), ("R", &top2[1])] {
        if branch.len() < 2 {
            continue;
        }
        let arc = |frac: f64| -> (f32, f32) {
            let idx = (((branch.len() - 1) as f64) * frac).round() as usize;
            let (px, py) = branch[idx.min(branch.len() - 1)];
            (px as f32, (py + top as i32) as f32)
        };
        let knee = arc(0.5);
        let ankle = arc(0.88);
        let foot = arc(1.0);
        let hip_pt = skel.joint(&format!("Thigh {side}")).map(|j| (j.x, j.y)).unwrap_or((hips.x, hips.y));

        for (name, pt) in [
            (format!("Knee {side}"), knee),
            (format!("Ankle {side}"), ankle),
            (format!("Foot {side}"), foot),
        ] {
            if let Some(j) = skel.joints.iter_mut().find(|j| j.name == name) {
                j.x = pt.0;
                j.y = pt.1;
                j.confidence = 0.5;
            }
        }
        // The foot bone is "Ankle {side}" (T10); a skeleton built elsewhere may still call it "Foot {side}".
        let (thigh_name, knee_name) = (format!("Thigh {side}"), format!("Knee {side}"));
        if let Some(b) = skel.bones.iter_mut().find(|b| b.name == thigh_name) {
            *b = make_bone(&thigh_name, hip_pt, knee);
        }
        if let Some(b) = skel.bones.iter_mut().find(|b| b.name == knee_name) {
            *b = make_bone(&knee_name, knee, ankle);
        }
        let foot_names = [format!("Ankle {side}"), format!("Foot {side}")];
        if let Some(b) = skel.bones.iter_mut().find(|b| foot_names.contains(&b.name)) {
            let own_name = b.name.clone();
            *b = make_bone(&own_name, ankle, foot);
        }
    }
    true
}

/// A single segmented body or face part mask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartMask {
    pub name: String,
    pub depth: i32,
    pub mask: Vec<u8>,
}

/// Collection of part masks for a character.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartMasks {
    pub width: u32,
    pub height: u32,
    pub parts: Vec<PartMask>,
    /// Every matte component disconnected from the character's own main
    /// silhouette (a detached hand, a held prop, a bowl set down a clear
    /// gap away) — kept out of `parts` so nothing in `parts` can be glued to
    /// the wrong bone by the nearest-bone Voronoi fill, but not discarded:
    /// `complete_parts`/`trace_character_whole` process these exactly like
    /// any other part (no bone means no capsule/joint-cap treatment), and
    /// `assemble_character` places each as its own unbound layer at its own
    /// position instead of rigging it. See `part_masks`.
    #[serde(default)]
    pub props: Vec<PartMask>,
}

impl PartMasks {
    pub fn get(&self, name: &str) -> Option<&PartMask> {
        self.parts.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// Renders a color-coded palette image showing each part with a distinct color.
    pub fn color_palette_image(&self) -> image::RgbaImage {
        let mut img = image::RgbaImage::new(self.width, self.height);
        let palette = [
            [230, 25, 75, 255],   // Red
            [60, 180, 75, 255],   // Green
            [255, 225, 25, 255],  // Yellow
            [0, 130, 200, 255],   // Blue
            [245, 130, 48, 255],  // Orange
            [145, 30, 180, 255],  // Purple
            [70, 240, 240, 255],  // Cyan
            [240, 50, 230, 255],  // Magenta
            [210, 245, 60, 255],  // Lime
            [250, 190, 212, 255], // Pink
            [0, 128, 128, 255],   // Teal
            [220, 190, 255, 255], // Lavender
            [170, 110, 40, 255],  // Brown
            [255, 250, 200, 255], // Beige
            [128, 0, 0, 255],     // Maroon
            [170, 255, 195, 255], // Mint
            [128, 128, 0, 255],   // Olive
            [255, 215, 180, 255], // Coral
            [0, 0, 128, 255],     // Navy
            [128, 128, 128, 255], // Grey
        ];

        let mut sorted: Vec<&PartMask> = self.parts.iter().collect();
        sorted.sort_by_key(|p| p.depth);

        for (color_idx, part) in sorted.iter().enumerate() {
            let col = palette[color_idx % palette.len()];
            for y in 0..self.height {
                for x in 0..self.width {
                    let idx = (y * self.width + x) as usize;
                    if part.mask[idx] > 128 {
                        img.put_pixel(x, y, image::Rgba(col));
                    }
                }
            }
        }
        img
    }

    /// Extracts an RGBA image containing only the specified part's pixels from `original_img`.
    pub fn part_alpha_image(&self, part_name: &str, original_img: &image::RgbaImage) -> Option<image::RgbaImage> {
        let part = self.get(part_name)?;
        let mut out = image::RgbaImage::new(self.width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = (y * self.width + x) as usize;
                let m = part.mask[idx];
                if m > 0 {
                    let mut p = *original_img.get_pixel(x, y);
                    p[3] = ((p[3] as u16 * m as u16) / 255) as u8;
                    out.put_pixel(x, y, p);
                }
            }
        }
        Some(out)
    }
}

/// Generates segmented part masks for each bone slot and facial feature.
///
/// Each bone slot is segmented by sampling positive points along the bone,
/// estimating the local limb width via Euclidean distance transform of `matte`,
/// and constructing a capsule region. Overlaps are resolved according to depth
/// and distance scores.
pub fn part_masks(
    img: &image::RgbaImage,
    matte: &image::GrayImage,
    skel: &Skeleton,
) -> anyhow::Result<PartMasks> {
    let (w, h) = (matte.width(), matte.height());
    if w == 0 || h == 0 {
        return Ok(PartMasks {
            width: w,
            height: h,
            parts: Vec::new(),
            props: Vec::new(),
        });
    }

    let dt = crate::segment::distance_transform(matte);
    let height = skel.character_height.max(10.0);

    // Bone slot definitions with depth order
    let bone_depth = |name: &str| -> i32 {
        let n = name.to_lowercase();
        if n.contains("head") {
            90
        } else if n.contains("shoulder l") || n.contains("elbow l") || n.contains("hand l") || n.contains("wrist l") {
            80
        } else if n.contains("chest") || n.contains("torso") {
            72
        } else if n.contains("hips") || n.contains("pelvis") {
            70
        } else if n.contains("thigh l") || n.contains("knee l") || n.contains("foot l") || n.contains("ankle l") {
            60
        } else if n.contains("thigh r") || n.contains("knee r") || n.contains("foot r") || n.contains("ankle r") {
            50
        } else if n.contains("shoulder r") || n.contains("elbow r") || n.contains("hand r") || n.contains("wrist r") {
            40
        } else {
            50
        }
    };

    struct PartCandidate {
        name: String,
        depth: i32,
        scores: Vec<f32>,
        is_face: bool,
        /// Real SAM2 mask confidence, when this candidate was SAM2-refined.
        sam_score: Option<f32>,
    }

    let mut candidates = Vec::new();

    // Bone geometry kept alongside `candidates`, for the SAM2 refinement
    // pass below (points/box prompts, and the 2-nearest-bone negative
    // points): name -> (head, tail, capsule radius, capsule box).
    let mut bone_geo: std::collections::HashMap<String, ((f32, f32), (f32, f32), f32, [f32; 4])> =
        std::collections::HashMap::new();

    // 1. Process each bone
    for bone in &skel.bones {
        if bone.length <= 1.0 {
            continue;
        }

        let (x0, y0) = bone.head;
        let (x1, y1) = bone.tail;
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len_sq = dx * dx + dy * dy;

        // Sample limb width along bone
        let mut width_sum = 0.0f32;
        let mut width_count = 0;
        for t in [0.2f32, 0.4, 0.6, 0.8] {
            let sx = (x0 + t * dx).clamp(0.0, (w - 1) as f32) as usize;
            let sy = (y0 + t * dy).clamp(0.0, (h - 1) as f32) as usize;
            let d = dt[sy * w as usize + sx];
            if d > 0.0 {
                width_sum += d;
                width_count += 1;
            }
        }
        let limb_width = if width_count > 0 {
            (width_sum / width_count as f32).max(5.0)
        } else {
            (height * 0.05).max(8.0)
        };
        let radius = limb_width * 0.75;
        let radius_sq = radius * radius;

        let mut scores = vec![0.0f32; (w * h) as usize];
        let min_x = ((x0.min(x1) - radius).floor().max(0.0)) as u32;
        let max_x = ((x0.max(x1) + radius).ceil().min((w - 1) as f32)) as u32;
        let min_y = ((y0.min(y1) - radius).floor().max(0.0)) as u32;
        let max_y = ((y0.max(y1) + radius).ceil().min((h - 1) as f32)) as u32;

        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let idx = (py * w + px) as usize;
                if matte.get_pixel(px, py)[0] <= 128 {
                    continue;
                }

                let px_f = px as f32;
                let py_f = py as f32;
                let u = ((px_f - x0) * dx + (py_f - y0) * dy) / len_sq;
                let t_clamp = u.clamp(0.0, 1.0);
                let proj_x = x0 + t_clamp * dx;
                let proj_y = y0 + t_clamp * dy;
                let dist_sq = (px_f - proj_x) * (px_f - proj_x) + (py_f - proj_y) * (py_f - proj_y);

                if dist_sq <= radius_sq {
                    let dist = dist_sq.sqrt();
                    scores[idx] = (1.0 - dist / radius).max(0.01);
                }
            }
        }

        bone_geo.insert(
            bone.name.clone(),
            (
                (x0, y0),
                (x1, y1),
                radius,
                [min_x as f32, min_y as f32, max_x as f32, max_y as f32],
            ),
        );

        candidates.push(PartCandidate {
            name: bone.name.clone(),
            depth: bone_depth(&bone.name),
            scores,
            is_face: false,
            sam_score: None,
        });
    }

    // 2. Process face parts
    let face_parts = [
        ("Left Eye", &skel.face.left_eye),
        ("Right Eye", &skel.face.right_eye),
        ("Left Eyebrow", &skel.face.left_brow),
        ("Right Eyebrow", &skel.face.right_brow),
        ("Mouth", &skel.face.mouth),
    ];

    for (name, pts) in face_parts {
        if pts.is_empty() {
            continue;
        }

        let radius = (height * 0.03).max(6.0);
        let radius_sq = radius * radius;
        let mut scores = vec![0.0f32; (w * h) as usize];

        for pt in pts {
            let (cx, cy, conf) = *pt;
            if conf < 0.2 {
                continue;
            }
            let min_x = ((cx - radius).floor().max(0.0)) as u32;
            let max_x = ((cx + radius).ceil().min((w - 1) as f32)) as u32;
            let min_y = ((cy - radius).floor().max(0.0)) as u32;
            let max_y = ((cy + radius).ceil().min((h - 1) as f32)) as u32;

            for py in min_y..=max_y {
                for px in min_x..=max_x {
                    let idx = (py * w + px) as usize;
                    if matte.get_pixel(px, py)[0] <= 128 {
                        continue;
                    }
                    let dx = px as f32 - cx;
                    let dy = py as f32 - cy;
                    let dist_sq = dx * dx + dy * dy;
                    if dist_sq <= radius_sq {
                        let dist = dist_sq.sqrt();
                        let s = (1.0 - dist / radius).max(0.01);
                        if s > scores[idx] {
                            scores[idx] = s;
                        }
                    }
                }
            }
        }

        // **An eye owns the eye, not the temple round it.** The discs above
        // (35 px on a 1178 px picture) around each of an eye's contour points
        // reach far past the eye, and the Head's own mask does not cover every
        // pixel of the temple, so the eye part took hair and skin beside the eye.
        // `assemble_character` drops everything an eye part draws (the synthetic
        // eye replaces it), which left those pixels with no paint at all: a white
        // wedge beside each eye, the "eye whites larger than the drawn eyes".
        // Keep only what lies within a little over one eye's width of the eye's
        // centre; the rest goes back to the Head in the gap fill below.
        if name.ends_with("Eye") {
            let confident: Vec<&(f32, f32, f32)> = pts.iter().filter(|p| p.2 >= 0.2).collect();
            if confident.len() >= 2 {
                let x0 = confident.iter().map(|p| p.0).fold(f32::MAX, f32::min);
                let x1 = confident.iter().map(|p| p.0).fold(f32::MIN, f32::max);
                let y0 = confident.iter().map(|p| p.1).fold(f32::MAX, f32::min);
                let y1 = confident.iter().map(|p| p.1).fold(f32::MIN, f32::max);
                let (ecx, ecy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
                let reach = 1.1 * (x1 - x0).max(y1 - y0) + 4.0;
                let reach_sq = reach * reach;
                for (idx, s) in scores.iter_mut().enumerate() {
                    if *s > 0.0 {
                        let (dx, dy) = ((idx as u32 % w) as f32 - ecx, (idx as u32 / w) as f32 - ecy);
                        if dx * dx + dy * dy > reach_sq {
                            *s = 0.0;
                        }
                    }
                }
            }
        }

        candidates.push(PartCandidate {
            name: name.to_string(),
            depth: 100,
            scores,
            is_face: true,
            sam_score: None,
        });
    }

    // 3. SAM2 refinement: prompt SAM2 per bone slot with points along the
    // bone, negative points on the 2 nearest other bones, and the capsule as
    // a box prompt. Final mask = SAM mask ∩ dilated capsule prior (to stop
    // leakage into neighbouring parts / clothing edges), replacing that
    // candidate's geometric-only scores. The image is encoded once, then
    // decoded per part. Falls back to the geometric capsule (`scores`
    // computed above) when SAM2 isn't installed or a call fails.
    if let Ok(Some(embeddings)) = segment::sam2_encode(img) {
        let midpoints: Vec<(String, (f32, f32))> = bone_geo
            .iter()
            .map(|(name, (head, tail, _, _))| (name.clone(), ((head.0 + tail.0) * 0.5, (head.1 + tail.1) * 0.5)))
            .collect();

        for cand in candidates.iter_mut() {
            let Some(&(head, tail, radius, capsule_box)) = bone_geo.get(&cand.name) else {
                continue;
            };
            let (mx, my) = ((head.0 + tail.0) * 0.5, (head.1 + tail.1) * 0.5);

            let mut others: Vec<(f32, (f32, f32))> = midpoints
                .iter()
                .filter(|(name, _)| name != &cand.name)
                .map(|(_, pt)| ((pt.0 - mx).powi(2) + (pt.1 - my).powi(2), *pt))
                .collect();
            others.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

            let mut points: Vec<Sam2Point> = Vec::new();
            for t in [0.2f32, 0.4, 0.6, 0.8] {
                points.push((head.0 + t * (tail.0 - head.0), head.1 + t * (tail.1 - head.1), true));
            }
            for (_, pt) in others.into_iter().take(2) {
                points.push((pt.0, pt.1, false));
            }

            match segment::sam2_mask(&embeddings, &points, Some(capsule_box)) {
                Ok(Some((sam_mask, score))) => {
                    cand.sam_score = Some(score);
                    let dilated_radius = radius * 1.5;
                    let dilated_radius_sq = dilated_radius * dilated_radius;
                    let dx = tail.0 - head.0;
                    let dy = tail.1 - head.1;
                    let len_sq = (dx * dx + dy * dy).max(1.0);

                    let min_x = ((head.0.min(tail.0) - dilated_radius).floor().max(0.0)) as u32;
                    let max_x = ((head.0.max(tail.0) + dilated_radius).ceil().min((w - 1) as f32)) as u32;
                    let min_y = ((head.1.min(tail.1) - dilated_radius).floor().max(0.0)) as u32;
                    let max_y = ((head.1.max(tail.1) + dilated_radius).ceil().min((h - 1) as f32)) as u32;

                    let mut refined = vec![0.0f32; (w * h) as usize];
                    for py in min_y..=max_y {
                        for px in min_x..=max_x {
                            let idx = (py * w + px) as usize;
                            if matte.get_pixel(px, py)[0] <= 128 || sam_mask.get_pixel(px, py)[0] <= 128 {
                                continue;
                            }
                            let px_f = px as f32;
                            let py_f = py as f32;
                            let u = ((px_f - head.0) * dx + (py_f - head.1) * dy) / len_sq;
                            let t_clamp = u.clamp(0.0, 1.0);
                            let proj_x = head.0 + t_clamp * dx;
                            let proj_y = head.1 + t_clamp * dy;
                            let dist_sq = (px_f - proj_x).powi(2) + (py_f - proj_y).powi(2);
                            if dist_sq <= dilated_radius_sq {
                                refined[idx] = 1.0;
                            }
                        }
                    }
                    cand.scores = refined;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("SAM2 mask failed for part {}: {e}", cand.name);
                }
            }
        }
    }

    // 4. Clean each candidate's own raw mask (item 2: SAM2 masks are
    // speckled/fragmented — morphological close then open, largest
    // component, fill small holes), then resolve per-pixel overlaps by the
    // higher SAM2 score (falling back to the old distance*depth rank for
    // candidates SAM2 didn't refine).
    let num_candidates = candidates.len();
    let cleaned: Vec<Vec<u8>> = candidates
        .iter()
        .map(|cand| {
            let raw: Vec<u8> = cand.scores.iter().map(|&s| if s > 0.0 { 255 } else { 0 }).collect();
            segment::clean_part_mask(&raw, w, h)
        })
        .collect();

    let mut label_map = vec![-1i32; (w * h) as usize];
    let mut claim_rank = vec![f32::MIN; (w * h) as usize];
    for (c_idx, cand) in candidates.iter().enumerate() {
        for idx in 0..(w * h) as usize {
            if cleaned[c_idx][idx] <= 128 {
                continue;
            }
            let rank = match cand.sam_score {
                Some(s) => s * 1000.0 + cand.depth as f32,
                None => cand.scores[idx] * 100.0 + cand.depth as f32,
            };
            if rank > claim_rank[idx] {
                claim_rank[idx] = rank;
                label_map[idx] = c_idx as i32;
            }
        }
    }

    // 5. Pixels no part claims (matte-interior gaps left by cleaning) go to
    // the nearest *bone* part — never a face sub-part — via a label-map
    // Voronoi assignment (a multi-label chamfer distance transform), so the
    // label map covers the whole matte with no gaps.
    let mut voronoi_buf: Vec<i32> = label_map
        .iter()
        .map(|&l| if l >= 0 && candidates[l as usize].is_face { -2 } else { l })
        .collect();
    segment::voronoi_labels(&mut voronoi_buf, matte, w, h);
    for idx in 0..label_map.len() {
        let is_face_claim = label_map[idx] >= 0 && candidates[label_map[idx] as usize].is_face;
        if !is_face_claim {
            label_map[idx] = voronoi_buf[idx];
        }
    }

    // 5b. **The hand stops at the wrist, the foot at the ankle** (T10). Forearm and
    // hand (shin and foot) are separate bones now (`Wrist L/R`, `Ankle L/R` ride their
    // own slots), but their capsules overlap round the joint, so which one owned a
    // pixel there was a matter of score and depth. The wrist (ankle) line decides
    // instead: the line through the joint, perpendicular to the forearm (shin) bone.
    // A pixel of either part beyond it (along the bone's direction) is the hand
    // (foot), one short of it is the forearm (shin). Pixels of any other part are left.
    for (proximal, distal) in [
        ("Elbow L", ["Wrist L", "Hand L"]),
        ("Elbow R", ["Wrist R", "Hand R"]),
        ("Knee L", ["Ankle L", "Foot L"]),
        ("Knee R", ["Ankle R", "Foot R"]),
    ] {
        let find = |names: &[&str]| {
            candidates
                .iter()
                .position(|c| !c.is_face && names.iter().any(|n| c.name.eq_ignore_ascii_case(n)))
        };
        let (Some(near), Some(far)) = (find(&[proximal]), find(&distal)) else { continue };
        let Some(&(head, tail, _, _)) = bone_geo.get(&candidates[near].name) else { continue };
        let (ax, ay) = (tail.0 - head.0, tail.1 - head.1);
        let axis_len = (ax * ax + ay * ay).sqrt();
        if axis_len < 1.0 {
            continue;
        }
        let (ax, ay) = (ax / axis_len, ay / axis_len);
        for (idx, label) in label_map.iter_mut().enumerate() {
            if *label != near as i32 && *label != far as i32 {
                continue;
            }
            let (px, py) = ((idx as u32 % w) as f32, (idx as u32 / w) as f32);
            let along = (px - tail.0) * ax + (py - tail.1) * ay;
            *label = if along >= 0.0 { far as i32 } else { near as i32 };
        }
    }

    // 6. A pixel in a *different* connected component from the character's
    // own main silhouette — a detached prop, a bowl set down a clear gap
    // away, a separately-drawn hand — must never end up in any bone/face
    // part: "nearest by chamfer distance" (item 5, above) is not "belongs
    // to", and gluing it onto whichever bone happens to be closest is what
    // produced the "floating hat" artefact. Every such component is pulled
    // out of the label map here and collected as its own unbound `Prop`
    // mask instead — kept, not discarded (`matte` already dropped genuine
    // specks; anything that survived that is real content) — for
    // `assemble_character` to place at its own position without rigging it.
    let mut matte_mask = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            if matte.get_pixel(x, y)[0] > 128 {
                matte_mask[(y * w + x) as usize] = 255;
            }
        }
    }
    let components = segment::connected_components(&matte_mask, w, h);
    let main_component = components.iter().enumerate().max_by_key(|(_, c)| c.len()).map(|(i, _)| i);

    let mut props: Vec<PartMask> = Vec::new();
    if let Some(main_idx) = main_component {
        let mut prop_components: Vec<&Vec<usize>> = components
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != main_idx)
            .map(|(_, c)| c)
            .collect();
        // Reading order (top-to-bottom, then left-to-right) for stable,
        // predictable naming.
        prop_components.sort_by_key(|c| *c.iter().min().unwrap());

        for (i, comp) in prop_components.into_iter().enumerate() {
            let mut mask = vec![0u8; (w * h) as usize];
            for &idx in comp {
                mask[idx] = 255;
                label_map[idx] = -1;
            }
            props.push(PartMask {
                name: format!("Prop {}", i + 1),
                depth: 200,
                mask,
            });
        }
    }

    let mut part_masks_data = vec![vec![0u8; (w * h) as usize]; num_candidates];
    for (idx, &lbl) in label_map.iter().enumerate() {
        if lbl >= 0 {
            part_masks_data[lbl as usize][idx] = 255;
        }
    }

    let parts = candidates
        .into_iter()
        .zip(part_masks_data)
        .map(|(cand, mask)| PartMask {
            name: cand.name,
            depth: cand.depth,
            mask,
        })
        .collect();

    Ok(PartMasks {
        width: w,
        height: h,
        parts,
        props,
    })
}

/// A completed character part with hidden regions filled and joint caps added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletedPart {
    pub name: String,
    pub depth: i32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// This part's own dominant fill colour (item 4), computed once here
    /// from the best evidence available — its own segmented pixels, or a
    /// growing local probe, or the whole character — and carried alongside
    /// the raster so `trace_character_whole`'s joint cap and close-the-cut
    /// fill can reuse it directly instead of re-deriving it from this
    /// part's own completed alpha, which can be sparse-to-empty right where
    /// a badly detected joint sits in empty background (the "pale disc at
    /// OldmanfrontWalk's foot" artefact: the *raster* completion correctly
    /// refuses to paint outside the character's silhouette there, but that
    /// leaves nothing for a same-part alpha re-sample to find).
    #[serde(default)]
    pub dom_color: [u8; 3],
    /// B1: the pixels [`crate::outpaint::outpaint_limb`] added (the tube under the
    /// neighbour that cuts this limb), `width × height`; empty when none.
    /// `trace_character_whole_with` traces them with no `reach` gate. In memory only.
    #[serde(skip)]
    pub extruded: Vec<bool>,
    /// B1b: the L/R counterpart whose completed picture this limb borrowed (it
    /// owned under 20 % of the counterpart's pixels; `outpaint::borrow_counterpart`,
    /// its borrowed pixels marked in `extruded`); `None` for every other part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub borrowed_from: Option<String>,
}

impl CompletedPart {
    pub fn to_image(&self) -> image::RgbaImage {
        image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
            .unwrap_or_else(|| image::RgbaImage::new(self.width, self.height))
    }
}

/// Collection of all completed parts for a character.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletedParts {
    pub width: u32,
    pub height: u32,
    pub parts: Vec<CompletedPart>,
}

impl CompletedParts {
    pub fn get(&self, name: &str) -> Option<&CompletedPart> {
        self.parts.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// Computes dominant non-line color of a part using k-means (k=3).
fn dominant_color(img: &image::RgbaImage, mask: &[u8]) -> [u8; 3] {
    let mut pixels = Vec::new();
    let (w, h) = (img.width(), img.height());
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if mask[idx] > 128 {
                let p = img.get_pixel(x, y);
                if p[3] > 128 {
                    // Filter out dark outline strokes (luminance < 35)
                    let lum = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
                    if lum >= 35.0 {
                        pixels.push([p[0] as f32, p[1] as f32, p[2] as f32]);
                    }
                }
            }
        }
    }

    if pixels.is_empty() {
        return [200, 200, 200];
    }
    if pixels.len() < 3 {
        return [pixels[0][0] as u8, pixels[0][1] as u8, pixels[0][2] as u8];
    }

    let mut centers = [
        pixels[0],
        pixels[pixels.len() / 2],
        pixels[pixels.len() - 1],
    ];

    for _ in 0..5 {
        let mut sums = [[0.0f32; 3]; 3];
        let mut counts = [0usize; 3];

        for p in &pixels {
            let mut best_k = 0;
            let mut min_d = 1e9f32;
            for (k, c) in centers.iter().enumerate() {
                let d = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2);
                if d < min_d {
                    min_d = d;
                    best_k = k;
                }
            }
            sums[best_k][0] += p[0];
            sums[best_k][1] += p[1];
            sums[best_k][2] += p[2];
            counts[best_k] += 1;
        }

        for k in 0..3 {
            if counts[k] > 0 {
                centers[k][0] = sums[k][0] / counts[k] as f32;
                centers[k][1] = sums[k][1] / counts[k] as f32;
                centers[k][2] = sums[k][2] / counts[k] as f32;
            }
        }
    }

    let mut counts = [0usize; 3];
    for p in &pixels {
        let mut best_k = 0;
        let mut min_d = 1e9f32;
        for (k, c) in centers.iter().enumerate() {
            let d = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2);
            if d < min_d {
                min_d = d;
                best_k = k;
            }
        }
        counts[best_k] += 1;
    }

    let mut best_cluster = 0;
    let mut max_count = counts[0];
    for k in 1..3 {
        if counts[k] > max_count {
            max_count = counts[k];
            best_cluster = k;
        }
    }

    [
        centers[best_cluster][0].clamp(0.0, 255.0) as u8,
        centers[best_cluster][1].clamp(0.0, 255.0) as u8,
        centers[best_cluster][2].clamp(0.0, 255.0) as u8,
    ]
}

/// Inpaints a part's newly-added hidden region with LaMa, cropped around the
/// hole and resized to LaMa's fixed 512x512 I/O, using the source `img` as
/// surrounding context. Returns a full `w x h` RGB buffer whose values are
/// only meaningful at `hole` positions, or `None` when LaMa isn't installed,
/// there's no hole, or inference fails (caller falls back to a flat fill).
fn lama_complete_hole(img: &image::RgbaImage, hole: &[bool], w: u32, h: u32) -> Option<Vec<[u8; 3]>> {
    if crate::onnx::pooled(crate::models::ModelId::LaMa).is_none() {
        return None;
    }

    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0u32, 0u32);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if hole[(y * w + x) as usize] {
                any = true;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    if !any {
        return None;
    }

    // A square crop around the hole, padded by 25% of its size (at least
    // 32px total half-width), shifted (not shrunk) to stay inside the image.
    let hole_w = (max_x - min_x + 1) as f32;
    let hole_h = (max_y - min_y + 1) as f32;
    let half = (hole_w.max(hole_h) * 0.625).max(32.0);
    let cx = (min_x as f32 + max_x as f32) / 2.0;
    let cy = (min_y as f32 + max_y as f32) / 2.0;

    let mut cx0 = cx - half;
    let mut cy0 = cy - half;
    let mut cx1 = cx + half;
    let mut cy1 = cy + half;
    if cx0 < 0.0 {
        cx1 -= cx0;
        cx0 = 0.0;
    }
    if cy0 < 0.0 {
        cy1 -= cy0;
        cy0 = 0.0;
    }
    if cx1 > w as f32 {
        let over = cx1 - w as f32;
        cx0 = (cx0 - over).max(0.0);
        cx1 = w as f32;
    }
    if cy1 > h as f32 {
        let over = cy1 - h as f32;
        cy0 = (cy0 - over).max(0.0);
        cy1 = h as f32;
    }
    let crop_x = cx0.floor() as u32;
    let crop_y = cy0.floor() as u32;
    let crop_w = ((cx1 - cx0).round() as u32).clamp(1, w - crop_x);
    let crop_h = ((cy1 - cy0).round() as u32).clamp(1, h - crop_y);

    let crop_img = image::imageops::crop_imm(img, crop_x, crop_y, crop_w, crop_h).to_image();
    let mut crop_mask = image::GrayImage::new(crop_w, crop_h);
    for y in 0..crop_h {
        for x in 0..crop_w {
            let (ox, oy) = (crop_x + x, crop_y + y);
            if hole[(oy * w + ox) as usize] {
                crop_mask.put_pixel(x, y, image::Luma([255]));
            }
        }
    }

    let resized_img = image::imageops::resize(&crop_img, 512, 512, image::imageops::FilterType::Triangle);
    let resized_mask = image::imageops::resize(&crop_mask, 512, 512, image::imageops::FilterType::Nearest);

    let filled = match segment::lama_inpaint_512(&resized_img, &resized_mask) {
        Ok(Some(f)) => f,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("LaMa inpainting failed: {e}");
            return None;
        }
    };

    let back = image::imageops::resize(&filled, crop_w, crop_h, image::imageops::FilterType::Triangle);
    let mut out = vec![[0u8; 3]; (w * h) as usize];
    for y in 0..crop_h {
        for x in 0..crop_w {
            let (ox, oy) = (crop_x + x, crop_y + y);
            let p = back.get_pixel(x, y);
            out[(oy * w + ox) as usize] = [p[0], p[1], p[2]];
        }
    }
    Some(out)
}

/// Completes hidden parts geometrically and fills in occluded areas and joint caps.
///
/// For each body part:
/// 1. Extends alpha across its bone capsule within the silhouette matte.
/// 2. Adds a round joint cap (disc of local limb width) at the parent joint.
/// 3. Inpaints or flat-fills the added region with the part's dominant color.
///
/// B1 (`docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md`): without LaMa, a limb is instead
/// extruded from its cut edge along the bone (`outpaint::outpaint_limb`, the tube
/// recorded in `CompletedPart::extruded`), and any other bone part's hole takes
/// its nearest visible colour (`outpaint::nearest_visible_fill`).
///
/// LaMa is **off by default** (item 4: "High-quality fill", off by default):
/// this fast path always flat-fills newly-added regions with the part's
/// dominant colour, which is what keeps the warm pipeline within the ≤10s
/// (without LaMa) target. Use [`complete_parts_high_quality`] to opt in to
/// LaMa, run only on parts whose fill colour is textured enough to need it —
/// intended to run afterwards, in the background, once the character from
/// this fast path is already usable.
pub fn complete_parts(
    img: &image::RgbaImage,
    matte: &image::GrayImage,
    skel: &Skeleton,
    part_masks: &PartMasks,
) -> anyhow::Result<CompletedParts> {
    complete_parts_impl(img, matte, skel, part_masks, None)
}

/// Same as [`complete_parts`], but additionally runs LaMa inpainting (CPU,
/// 20-30s per character) on any part whose visible fill colour variance
/// exceeds `variance_threshold` — a proxy for "this part is textured, a flat
/// fill would look wrong". Meant to be called as a follow-up, optional,
/// background pass; see [`complete_parts`]'s doc comment.
pub fn complete_parts_high_quality(
    img: &image::RgbaImage,
    matte: &image::GrayImage,
    skel: &Skeleton,
    part_masks: &PartMasks,
    variance_threshold: f32,
) -> anyhow::Result<CompletedParts> {
    complete_parts_impl(img, matte, skel, part_masks, Some(variance_threshold))
}

/// Sample variance of the RGB fill colour under `mask` (ignoring dark line
/// pixels, the same filter [`dominant_color`] uses), as a proxy for how
/// textured a part's fill is. High variance means a flat dominant-colour
/// fill would look visibly wrong, and LaMa is worth its cost.
fn fill_colour_variance(img: &image::RgbaImage, mask: &[u8]) -> f32 {
    let (w, h) = (img.width(), img.height());
    let mut samples: Vec<[f32; 3]> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if mask[idx] <= 128 {
                continue;
            }
            let p = img.get_pixel(x, y);
            if p[3] <= 128 {
                continue;
            }
            let lum = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
            if lum < 35.0 {
                continue;
            }
            samples.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
    }
    if samples.len() < 2 {
        return 0.0;
    }
    let n = samples.len() as f32;
    let mean = samples.iter().fold([0.0f32; 3], |acc, p| [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]])
        .map(|s| s / n);
    let var = samples
        .iter()
        .map(|p| (p[0] - mean[0]).powi(2) + (p[1] - mean[1]).powi(2) + (p[2] - mean[2]).powi(2))
        .sum::<f32>()
        / n;
    var
}

fn complete_parts_impl(
    img: &image::RgbaImage,
    matte: &image::GrayImage,
    skel: &Skeleton,
    part_masks: &PartMasks,
    lama_variance_threshold: Option<f32>,
) -> anyhow::Result<CompletedParts> {
    let (w, h) = (matte.width(), matte.height());
    let dt = crate::segment::distance_transform(matte);
    let height = skel.character_height.max(10.0);

    // **Who paints over whom, per pixel** — what "hidden in the source" means.
    // The part masks are a partition of the matte (every character pixel has
    // exactly one visible owner), and `trace_character_whole` composites the
    // parts in `(depth, list position)` order, last one on top. A part's
    // capsule/joint-cap completion may therefore only fill a pixel whose
    // visible owner paints *over* that part (the part is genuinely hidden
    // there): the fill is then under the owner and never seen at rest, and
    // only shows if the owner turns away. A pixel owned by a part that sits
    // *under* this one (a lower depth, or the same depth earlier in the list)
    // is real, visible artwork of that other part, and the old unconditional
    // fill flat-painted it over in this part's dominant colour: the pale
    // half-discs across the chest and hips and the grey discs at the ankles
    // of every rig made from a photo-like drawing. `owner_rank[i]` is the
    // (depth, position) of pixel `i`'s owner, `None` where no part owns it.
    //
    // It is computed after B1b's paint order and B1c's straight cuts below.
    let all_parts: Vec<&PartMask> = part_masks.parts.iter().chain(part_masks.props.iter()).collect();
    let n_px = (w * h) as usize;
    let (wu, hu) = (w as usize, h as usize);
    let part_index =
        |name: &str| all_parts.iter().position(|p| p.name.eq_ignore_ascii_case(name) && skel.bone(name).is_some());
    // A limb slot's part and its name: the hand and foot bones are "Wrist L" /
    // "Ankle L" from `skeleton` and "Hand L" / "Foot L" from the wizard's edited
    // joints (`apply_edited_joints`), as the T10 wrist line in `part_masks` reads them.
    let slot_index = |slot: &str, side: &str| -> Option<(usize, String)> {
        let alias = match slot {
            "Wrist" => Some("Hand"),
            "Ankle" => Some("Foot"),
            _ => None,
        };
        std::iter::once(slot).chain(alias).find_map(|s| {
            let name = format!("{s} {side}");
            part_index(&name).map(|i| (i, name))
        })
    };

    // **B1b** (docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md): a limb owning under 20 % of
    // its L/R counterpart's pixels borrows the counterpart's completed picture
    // (after the loop below), and its whole side's chain (Thigh/Knee/Ankle or
    // Shoulder/Elbow/Wrist) is moved behind the counterpart chain (depth = the
    // counterpart chain's - 1) before anything reads the paint order: the borrowed
    // picture lies behind the near limb everywhere, the near limb keeps its
    // visible pixels, and the borrowed part's hidden pixels are everything the
    // near limb covers.
    let mut depths: Vec<i32> = all_parts.iter().map(|p| p.depth).collect();
    let px_count: Vec<usize> = all_parts.iter().map(|p| p.mask.iter().filter(|&&v| v > 128).count()).collect();
    let mut borrow_src: Vec<Option<usize>> = vec![None; all_parts.len()];
    let mut behind: Vec<bool> = vec![false; all_parts.len()];
    for chain in [["Thigh", "Knee", "Ankle"], ["Shoulder", "Elbow", "Wrist"]] {
        let side = |s: &str| -> Vec<usize> { chain.iter().filter_map(|slot| slot_index(slot, s).map(|(i, _)| i)).collect() };
        let (left, right) = (side("L"), side("R"));
        let mut triggered = [false; 2];
        for slot in chain {
            let (Some((l, _)), Some((r, _))) = (slot_index(slot, "L"), slot_index(slot, "R")) else {
                continue;
            };
            if crate::outpaint::borrows_counterpart(px_count[l], px_count[r]) {
                borrow_src[l] = Some(r);
                triggered[0] = true;
            }
            if crate::outpaint::borrows_counterpart(px_count[r], px_count[l]) {
                borrow_src[r] = Some(l);
                triggered[1] = true;
            }
        }
        let back_is_left = match triggered {
            [true, false] => Some(true),
            [false, true] => Some(false),
            // Both sides borrow (different slots): the side owning fewer pixels goes behind.
            [true, true] => Some(
                left.iter().map(|&i| px_count[i]).sum::<usize>() <= right.iter().map(|&i| px_count[i]).sum::<usize>(),
            ),
            [false, false] => None,
        };
        if let Some(back_is_left) = back_is_left {
            let (back, front) = if back_is_left { (&left, &right) } else { (&right, &left) };
            if let Some(front_depth) = front.iter().map(|&i| depths[i]).min() {
                for &i in back {
                    depths[i] = front_depth - 1;
                    behind[i] = true;
                }
            }
        }
    }

    // **B1c**: a clean, straight cut at every limb joint (Chest->Shoulder,
    // Shoulder->Elbow, Elbow->Wrist, Hips->Thigh, Thigh->Knee, Knee->Ankle, both
    // sides), perpendicular to the joint's child bone, at `t_cut` measured on the
    // profile of the part that paints over the other there
    // (`outpaint::measure_joint_cut`). Within the limb's band the two parts'
    // pixels are re-partitioned by side of the cut (`outpaint::apply_joint_cut`):
    // the over part keeps a straight cut end, the under part gets the strip as its
    // own visible pixels and, when the over part covers it, is extruded under it
    // to past the joint (B1). Two children of one parent (the thighs on the hips,
    // the arms on the chest) contest a parent pixel both cuts would hand over: a
    // chain moved behind by B1b loses to the near one, else the nearer bone axis
    // wins (then the earlier joint).
    let mut label = vec![-1i32; n_px];
    for (position, part) in all_parts.iter().enumerate() {
        for (i, &v) in part.mask.iter().enumerate() {
            if v > 128 {
                label[i] = position as i32;
            }
        }
    }
    let label0 = label.clone();
    let matte_in: Vec<bool> = matte.as_raw().iter().map(|&v| v > 128).collect();
    let rank = |position: usize| (depths[position], position);
    struct LimbJoint {
        parent: usize,
        child: usize,
        over_is_child: bool,
        cut: crate::outpaint::JointCut,
    }
    let mut joints: Vec<LimbJoint> = Vec::new();
    // `BUZZ_OUTPAINT_LOG`: each borrowing limb and each joint's cut, on stderr.
    let log = std::env::var_os("BUZZ_OUTPAINT_LOG").is_some();
    for side in ["L", "R"] {
        for (parent, child) in
            [("Chest", "Shoulder"), ("Shoulder", "Elbow"), ("Elbow", "Wrist"), ("Hips", "Thigh"), ("Thigh", "Knee"), ("Knee", "Ankle")]
        {
            let parent_slot = if parent == "Chest" || parent == "Hips" {
                part_index(parent).map(|i| (i, parent.to_string()))
            } else {
                slot_index(parent, side)
            };
            let (Some((p, parent_name)), Some((c, child_name))) = (parent_slot, slot_index(child, side)) else {
                if log {
                    eprintln!("outpaint: cut {parent} / {child} {side}: no such part or bone");
                }
                continue;
            };
            let (Some(bone), Some(parent_bone)) = (skel.bone(&child_name), skel.bone(&parent_name)) else {
                continue;
            };
            // A limb joint folded back on itself (the child bone turned more than 90°
            // from the parent limb's bone: a forearm raised alongside its upper arm, a
            // kneeling shin) has no cut across the limb: the two parts lie side by side.
            let (pd, cd) = (
                (parent_bone.tail.0 - parent_bone.head.0, parent_bone.tail.1 - parent_bone.head.1),
                (bone.tail.0 - bone.head.0, bone.tail.1 - bone.head.1),
            );
            if parent != "Chest" && parent != "Hips" && pd.0 * cd.0 + pd.1 * cd.1 < 0.0 {
                if log {
                    eprintln!("outpaint: cut {parent_name} / {child_name}: none, the joint is folded back on itself");
                }
                continue;
            }
            let over_is_child = rank(c) > rank(p);
            let over = if over_is_child { c } else { p };
            if let Some(cut) = crate::outpaint::measure_joint_cut(
                &label,
                wu,
                hu,
                &matte_in,
                over as i32,
                c as i32,
                bone.head,
                bone.tail,
                (parent_bone.head, parent_bone.tail),
            ) {
                joints.push(LimbJoint { parent: p, child: c, over_is_child, cut });
            } else if log {
                eprintln!("outpaint: cut {parent_name} / {child_name}: none (no limb pixel mid-bone, or no over pixel past the joint)");
            }
        }
    }
    if log {
        let borrowers: Vec<String> = borrow_src
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.map(|s| {
                    format!(
                        "{} from {} ({} px < 20 % of {} px; depth {} -> {})",
                        all_parts[i].name, all_parts[s].name, px_count[i], px_count[s], all_parts[i].depth, depths[i]
                    )
                })
            })
            .collect();
        eprintln!("outpaint: borrowed_from: {}", if borrowers.is_empty() { "none".to_string() } else { borrowers.join("; ") });
    }
    for (j, joint) in joints.iter().enumerate() {
        let moved = crate::outpaint::apply_joint_cut(
            &mut label,
            wu,
            hu,
            &joint.cut,
            joint.parent as i32,
            joint.child as i32,
            |x, y, s| {
                joints.iter().enumerate().any(|(k, other)| {
                    k != j
                        && other.parent == joint.parent
                        && other.child != joint.child
                        && other.cut.far_side_distance(x, y).is_some_and(|s_other| {
                            match (behind[joint.child], behind[other.child]) {
                                (true, false) => true,
                                (false, true) => false,
                                _ => s_other < s || (s_other == s && k < j),
                            }
                        })
                })
            },
        );
        if log {
            eprintln!(
                "outpaint: cut {} / {}: over {}, t_cut {} px, {} px moved",
                all_parts[joint.parent].name,
                all_parts[joint.child].name,
                all_parts[if joint.over_is_child { joint.child } else { joint.parent }].name,
                joint.cut.t_cut,
                moved
            );
        }
    }
    // The parts' own pixels after the cuts (a part no cut touched keeps its mask).
    let mut own_masks: Vec<Option<Vec<u8>>> = vec![None; all_parts.len()];
    for i in 0..n_px {
        let (from, to) = (label0[i], label[i]);
        if from == to {
            continue;
        }
        if from >= 0 {
            own_masks[from as usize].get_or_insert_with(|| all_parts[from as usize].mask.clone())[i] = 0;
        }
        if to >= 0 {
            own_masks[to as usize].get_or_insert_with(|| all_parts[to as usize].mask.clone())[i] = 255;
        }
    }
    let owner_rank: Vec<Option<(i32, usize)>> = label.iter().map(|&l| (l >= 0).then(|| rank(l as usize))).collect();

    let mut completed = Vec::new();

    // Props ride along in the same pass as every bone/face part: none of
    // them match a bone name (`skel.bone` returns `None`), so they fall
    // straight through to the "just copy this part's own pixels" path below
    // with no capsule/joint-cap completion — which is exactly right, since
    // a prop is never rigged.
    for (position, part) in part_masks.parts.iter().chain(part_masks.props.iter()).enumerate() {
        // B1b / B1c: this part's pixels after the straight cuts, and its paint order.
        let mask: &[u8] = own_masks[position].as_deref().unwrap_or(&part.mask);
        let depth = depths[position];
        let bone_opt = skel.bone(&part.name);
        // When this part's own C2 segmentation mask has no pixels at all
        // (fully occluded in this pose — e.g. a foot hidden behind a
        // trouser leg), `dominant_color` has nothing to sample and returns
        // its generic pale-grey `[200, 200, 200]` "no pixels" default. That
        // default then gets flat-filled across the whole capsule/joint-cap
        // hole below, with nothing to do with the character at all — the
        // "pale disc at OldmanfrontWalk's foot" artefact. Fall back to
        // sampling the real image within the character's own silhouette
        // near the bone's own midpoint instead: still an approximation,
        // but grounded in this character's actual nearby colours rather
        // than a fixed grey.
        let mask_has_pixels = mask.iter().any(|&v| v > 128);
        let dom_color = if mask_has_pixels {
            dominant_color(img, mask)
        } else {
            // No segmented pixels for this part at all (fully occluded in
            // this pose). Probe a growing box around the bone's own
            // midpoint, restricted to the character's silhouette, so a
            // local sample is preferred when one exists; but a walking
            // pose's stride gap between two separated legs can itself sit
            // entirely in empty background around the joint (the "pale
            // disc at OldmanfrontWalk's foot" artefact was a joint cap
            // centred in exactly such a gap, so even the local probe found
            // nothing), so this keeps growing — all the way out to the
            // whole matte — until it actually finds real character pixels,
            // rather than falling through to `dominant_color`'s generic
            // pale-grey "no pixels" default.
            let center = bone_opt.map(|b| {
                (((b.head.0 + b.tail.0) * 0.5).round() as i32, ((b.head.1 + b.tail.1) * 0.5).round() as i32)
            });
            let mut probed: Option<[u8; 3]> = None;
            if let Some((mx, my)) = center {
                for r in [20i32, 40, 80, 160] {
                    let mut probe_mask = vec![0u8; (w * h) as usize];
                    let mut any = false;
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let (px, py) = (mx + dx, my + dy);
                            if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                                let (pxu, pyu) = (px as u32, py as u32);
                                if matte.get_pixel(pxu, pyu)[0] > 128 {
                                    probe_mask[(pyu * w + pxu) as usize] = 255;
                                    any = true;
                                }
                            }
                        }
                    }
                    if any {
                        probed = Some(dominant_color(img, &probe_mask));
                        break;
                    }
                }
            }
            probed.unwrap_or_else(|| {
                // Ultimate fallback: the whole character's own dominant
                // colour is still grounded in this character, unlike a
                // fixed grey.
                let whole_mask: Vec<u8> = matte.pixels().map(|p| if p[0] > 128 { 255 } else { 0 }).collect();
                dominant_color(img, &whole_mask)
            })
        };

        let mut out_rgba = vec![0u8; (w * h * 4) as usize];
        let mut extruded: Vec<bool> = Vec::new();

        // 1. Copy visible pixels
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) as usize;
                if mask[idx] > 128 {
                    let p = img.get_pixel(x, y);
                    let out_idx = idx * 4;
                    out_rgba[out_idx] = p[0];
                    out_rgba[out_idx + 1] = p[1];
                    out_rgba[out_idx + 2] = p[2];
                    out_rgba[out_idx + 3] = p[3];
                }
            }
        }

        // 2. If it's a bone limb, compute capsule completion + joint cap
        if let Some(bone) = bone_opt {
            let (x0, y0) = bone.head;
            let (x1, y1) = bone.tail;
            let dx = x1 - x0;
            let dy = y1 - y0;
            let len_sq = (dx * dx + dy * dy).max(1.0);

            // Local limb width
            let mut width_sum = 0.0f32;
            let mut width_count = 0;
            for t in [0.2f32, 0.4, 0.6, 0.8] {
                let sx = (x0 + t * dx).clamp(0.0, (w - 1) as f32) as usize;
                let sy = (y0 + t * dy).clamp(0.0, (h - 1) as f32) as usize;
                let d = dt[sy * w as usize + sx];
                if d > 0.0 {
                    width_sum += d;
                    width_count += 1;
                }
            }
            let limb_width = if width_count > 0 {
                (width_sum / width_count as f32).max(5.0)
            } else {
                (height * 0.05).max(8.0)
            };
            // Cap at the bone's own length (see `limb_radius_at_head`'s
            // doc comment): without it, a short but *wide* torso bone
            // (Hips, Chest) reads the matte's distance transform near its
            // own centreline, which is close to half the character's whole
            // width, not half this bone's width — ballooning the capsule
            // and joint cap far past the bone (a "Hips" completion reaching
            // past the knee), stealing ownership from the leg parts
            // beneath it in the whole-character composite and painting
            // whatever line/shadow geometry sits there in Hips' place: the
            // stray dark filled band across pujariFront's kurta hem.
            let bone_len = (len_sq.max(1.0)).sqrt();
            let radius = (limb_width * 0.75).min(bone_len);
            let radius_sq = radius * radius;

            let min_x = ((x0.min(x1) - radius).floor().max(0.0)) as u32;
            let max_x = ((x0.max(x1) + radius).ceil().min((w - 1) as f32)) as u32;
            let min_y = ((y0.min(y1) - radius).floor().max(0.0)) as u32;
            let max_y = ((y0.max(y1) + radius).ceil().min((h - 1) as f32)) as u32;

            // First pass: which pixels the capsule/joint cap would newly
            // add (not already covered by visible pixels).
            let mut hole = vec![false; (w * h) as usize];
            let mut any_hole = false;
            for py in min_y..=max_y {
                for px in min_x..=max_x {
                    let idx = (py * w + px) as usize;
                    if matte.get_pixel(px, py)[0] <= 128 {
                        continue;
                    }

                    let px_f = px as f32;
                    let py_f = py as f32;

                    // Distance to bone segment
                    let u = ((px_f - x0) * dx + (py_f - y0) * dy) / len_sq;
                    let t_clamp = u.clamp(0.0, 1.0);
                    let proj_x = x0 + t_clamp * dx;
                    let proj_y = y0 + t_clamp * dy;
                    let dist_sq = (px_f - proj_x).powi(2) + (py_f - proj_y).powi(2);

                    // Distance to parent joint head (round joint cap)
                    let head_dist_sq = (px_f - x0).powi(2) + (py_f - y0).powi(2);

                    let inside_capsule = dist_sq <= radius_sq;
                    let inside_joint_cap = head_dist_sq <= radius_sq;

                    // Hidden in the source: no owner (nothing to paint over),
                    // or an owner that paints over this part. See `owner_rank`.
                    let hidden = owner_rank[idx].is_none_or(|owner| owner > (depth, position));

                    if (inside_capsule || inside_joint_cap) && hidden && out_rgba[idx * 4 + 3] == 0 {
                        hole[idx] = true;
                        any_hole = true;
                    }
                }
            }

            // Second pass: fill the hole. LaMa inpaints it from surrounding
            // image context, only when the caller opted in
            // (`lama_variance_threshold`) and this part's fill is textured
            // enough to need it; a flat fill in the part's dominant colour
            // is the default (and the fallback when LaMa is unavailable, no
            // hole, or inference failed).
            let wants_lama = any_hole
                && lama_variance_threshold
                    .is_some_and(|threshold| fill_colour_variance(img, mask) > threshold);
            let lama_fill = if wants_lama {
                lama_complete_hole(img, &hole, w, h)
            } else {
                None
            };

            if let Some(lama_fill) = lama_fill.as_ref() {
                for py in min_y..=max_y {
                    for px in min_x..=max_x {
                        let idx = (py * w + px) as usize;
                        if !hole[idx] {
                            continue;
                        }
                        let out_idx = idx * 4;
                        let rgb = lama_fill[idx];
                        out_rgba[out_idx] = rgb[0];
                        out_rgba[out_idx + 1] = rgb[1];
                        out_rgba[out_idx + 2] = rgb[2];
                        out_rgba[out_idx + 3] = 255;
                    }
                }
            } else {
                // B1: a limb is extruded from its cut edge along the bone
                // (`outpaint_limb`, on every pixel the hole rule allows, not just
                // the capsule); any other bone part, or a limb with no visible
                // pixel to take a cross-section from, fills the capsule hole with
                // the part's nearest visible colour (`dom_color` if it has none).
                let outpainted = crate::outpaint::is_limb_bone(&bone.name).then(|| {
                    let matte_raw = matte.as_raw();
                    let allowed: Vec<bool> = (0..(w * h) as usize)
                        .map(|idx| {
                            matte_raw[idx] > 128
                                && out_rgba[idx * 4 + 3] == 0
                                && owner_rank[idx].is_none_or(|owner| owner > (depth, position))
                        })
                        .collect();
                    crate::outpaint::outpaint_limb(&out_rgba, &allowed, &matte_in, bone.head, bone.tail, img)
                });
                match outpainted.flatten() {
                    Some(limb) => {
                        out_rgba = limb.rgba;
                        if limb.extruded.iter().any(|&e| e) {
                            extruded = limb.extruded;
                        }
                    }
                    None => crate::outpaint::nearest_visible_fill(&mut out_rgba, w, h, &hole, dom_color),
                }
            }
        }

        completed.push(CompletedPart {
            name: part.name.clone(),
            depth,
            width: w,
            height: h,
            rgba: out_rgba,
            dom_color,
            extruded,
            borrowed_from: None,
        });
    }

    // B1b: a borrowing limb's picture is its counterpart's completed picture,
    // mapped bone to bone, on every pixel it may paint (hidden under a part that
    // paints over it, inside the matte); its own visible pixels stay on top.
    for (position, src) in borrow_src.iter().enumerate() {
        let Some(src) = *src else { continue };
        let (Some(own), Some(other)) = (skel.bone(&all_parts[position].name), skel.bone(&all_parts[src].name)) else {
            continue;
        };
        let mask: &[u8] = own_masks[position].as_deref().unwrap_or(&all_parts[position].mask);
        let allowed: Vec<bool> =
            (0..n_px).map(|i| matte_in[i] && owner_rank[i].is_none_or(|owner| owner > rank(position))).collect();
        let (rgba, borrowed) = crate::outpaint::borrow_counterpart(
            mask,
            img,
            &allowed,
            &completed[src].rgba,
            (own.head, own.tail),
            (other.head, other.tail),
        );
        let part = &mut completed[position];
        part.rgba = rgba;
        part.extruded = if borrowed.iter().any(|&b| b) { borrowed } else { Vec::new() };
        part.borrowed_from = Some(all_parts[src].name.clone());
    }

    Ok(CompletedParts {
        width: w,
        height: h,
        parts: completed,
    })
}

/// Traced vector shapes for a single character part.
#[derive(Debug, Clone, PartialEq)]
pub struct TracedPart {
    pub name: String,
    pub depth: i32,
    pub shapes: Vec<buzz_scene::ShapeData>,
    /// The part's **ink**: the outlines and line work of the drawing as smooth,
    /// variable-width brush fills (`TraceMode::LineArt`, `LineStyle::Brush`),
    /// drawn after `shapes` so they sit on top of the fills they outline. The
    /// fills in `shapes` run *under* the ink (the ink's pixels are painted with
    /// the colour beside them before the fills are traced). Empty when the
    /// picture has no ink or the boundary-stroke option is on.
    /// `assemble_character` puts them in an "Ink" layer above the part's
    /// "Artwork", in the same part symbol.
    pub ink: Vec<buzz_scene::ShapeData>,
    /// The part's joint cap (a filled disc at its head joint and the outline of
    /// its exposed arc), **kept apart from `shapes`** because it does not belong
    /// at this part's depth: a cap only bridges the gap that opens when the part
    /// turns, so it must sit *behind both parts the joint joins*.
    /// `assemble_character` binds it to the part's bone as a rig part at the very
    /// back. Drawn inside `shapes` (as it once was) the disc lay over whatever
    /// part sat below this one in depth: the pale discs across the chest, hips
    /// and knees. Empty for a part with no bone (face parts, props) and for a
    /// joint outside the character.
    pub caps: Vec<buzz_scene::ShapeData>,
}

/// Fully vectorized character with parts, quantized palette, and uniform line width.
#[derive(Debug, Clone, PartialEq)]
pub struct TracedCharacter {
    pub parts: Vec<TracedPart>,
    pub palette: Vec<peniko::Color>,
    pub uniform_width: f64,
}

/// Quantizes the character-wide color palette using k-means (k = 6..12, default 8).
pub fn quantize_character_palette(completed: &CompletedParts, num_colors: usize) -> Vec<peniko::Color> {
    let k = num_colors.clamp(6, 32);
    let mut samples: Vec<[f32; 3]> = Vec::new();

    for part in &completed.parts {
        let step = (part.rgba.len() / (4 * 500)).max(1);
        for i in (0..part.rgba.len()).step_by(step * 4) {
            let a = part.rgba[i + 3];
            if a > 128 {
                samples.push([
                    part.rgba[i] as f32,
                    part.rgba[i + 1] as f32,
                    part.rgba[i + 2] as f32,
                ]);
            }
        }
    }

    if samples.is_empty() {
        return vec![
            peniko::Color::from_rgb8(20, 20, 20),
            peniko::Color::from_rgb8(200, 200, 200),
        ];
    }

    let step = (samples.len() / k).max(1);
    let mut centers: Vec<[f32; 3]> = (0..k)
        .map(|i| samples[(i * step).min(samples.len() - 1)])
        .collect();

    for _ in 0..8 {
        let mut sums = vec![[0.0f32; 3]; k];
        let mut counts = vec![0usize; k];

        for p in &samples {
            let mut best_k = 0;
            let mut min_d = 1e9f32;
            for (ci, c) in centers.iter().enumerate() {
                let d = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2);
                if d < min_d {
                    min_d = d;
                    best_k = ci;
                }
            }
            sums[best_k][0] += p[0];
            sums[best_k][1] += p[1];
            sums[best_k][2] += p[2];
            counts[best_k] += 1;
        }

        for ci in 0..k {
            if counts[ci] > 0 {
                centers[ci][0] = sums[ci][0] / counts[ci] as f32;
                centers[ci][1] = sums[ci][1] / counts[ci] as f32;
                centers[ci][2] = sums[ci][2] / counts[ci] as f32;
            }
        }
    }

    centers.sort_by(|a, b| {
        let lum_a = 0.299 * a[0] + 0.587 * a[1] + 0.114 * a[2];
        let lum_b = 0.299 * b[0] + 0.587 * b[1] + 0.114 * b[2];
        lum_a.partial_cmp(&lum_b).unwrap_or(std::cmp::Ordering::Equal)
    });

    centers
        .into_iter()
        .map(|c| {
            peniko::Color::from_rgba8(
                c[0].clamp(0.0, 255.0) as u8,
                c[1].clamp(0.0, 255.0) as u8,
                c[2].clamp(0.0, 255.0) as u8,
                255,
            )
        })
        .collect()
}

/// Traces completed character parts into vector artwork with uniform line width and quantized palette.
///
/// Superseded by `trace_character_whole`; per-part tracing loses faces. Kept for its own tests.
pub fn trace_character(
    completed: &CompletedParts,
    uniform_width: Option<f64>,
) -> anyhow::Result<TracedCharacter> {
    let palette = quantize_character_palette(completed, 8);
    let line_color = palette.first().copied().unwrap_or(peniko::Color::from_rgb8(15, 15, 15));
    let uw = uniform_width.unwrap_or(2.0);

    let mut traced_parts = Vec::new();

    for part in &completed.parts {
        let options = buzz_scene::trace::TraceOptions {
            mode: buzz_scene::trace::TraceMode::LineArt,
            line_style: buzz_scene::trace::LineStyle::Stroke,
            uniform_width: Some(uw),
            line_smooth: 0.6,
            fill_enclosed: true,
            colours: palette.len(),
            line_colour: Some(line_color),
            ..Default::default()
        };

        let report = buzz_scene::trace::trace(part.width, part.height, &part.rgba, &options);

        let mut fills = Vec::new();
        let mut strokes = Vec::new();

        for mut shape in report.shapes {
            if let Some(fill) = &mut shape.fill {
                let fc = fill.color();
                let c1 = fc.to_rgba8();
                let mut best_col = palette[0];
                let mut min_diff = 1e9f32;
                for p_col in &palette {
                    let c2 = p_col.to_rgba8();
                    let dr = (c1.r as f32 - c2.r as f32).powi(2);
                    let dg = (c1.g as f32 - c2.g as f32).powi(2);
                    let db = (c1.b as f32 - c2.b as f32).powi(2);
                    let diff = dr + dg + db;
                    if diff < min_diff {
                        min_diff = diff;
                        best_col = *p_col;
                    }
                }
                fill.paint = buzz_scene::object::Paint::Solid(best_col);
                fills.push(shape);
            } else if shape.stroke.is_some() {
                strokes.push(shape);
            }
        }

        // Trace part silhouette for base fill and outer closing stroke
        let total_px = (part.width * part.height) as usize;
        let mut sil_buf = vec![0u8; total_px * 4];
        let mut has_silhouette = false;
        for i in 0..total_px {
            if part.rgba[i * 4 + 3] > 128 {
                sil_buf[i * 4] = 0;
                sil_buf[i * 4 + 1] = 0;
                sil_buf[i * 4 + 2] = 0;
                sil_buf[i * 4 + 3] = 255;
                has_silhouette = true;
            }
        }

        let mut base_fills = Vec::new();
        let mut sil_strokes = Vec::new();

        if has_silhouette {
            let sil_opts = buzz_scene::trace::TraceOptions {
                mode: buzz_scene::trace::TraceMode::Outline,
                smooth: 0.6,
                speckle: 8,
                ..Default::default()
            };
            let sil_report = buzz_scene::trace::trace(part.width, part.height, &sil_buf, &sil_opts);

            // Dominant non-line color of the part snapped to palette
            let part_img = part.to_image();
            let mask = vec![255u8; total_px];
            let dom_raw = dominant_color(&part_img, &mask);
            let mut best_dom = palette[0];
            let mut min_diff = 1e9f32;
            for p_col in &palette {
                let c2 = p_col.to_rgba8();
                let dr = (dom_raw[0] as f32 - c2.r as f32).powi(2);
                let dg = (dom_raw[1] as f32 - c2.g as f32).powi(2);
                let db = (dom_raw[2] as f32 - c2.b as f32).powi(2);
                let diff = dr + dg + db;
                if diff < min_diff {
                    min_diff = diff;
                    best_dom = *p_col;
                }
            }

            for shape in sil_report.shapes {
                base_fills.push(buzz_scene::ShapeData::filled(shape.path.clone(), best_dom));
                sil_strokes.push(buzz_scene::ShapeData::stroked(shape.path, line_color, uw));
            }
        }

        // Layer ordering: base fills, enclosed fills, ink strokes, outer closing strokes
        let mut shapes = base_fills;
        shapes.extend(fills);
        shapes.extend(strokes);
        shapes.extend(sil_strokes);

        traced_parts.push(TracedPart {
            name: part.name.clone(),
            depth: part.depth,
            shapes,
            ink: Vec::new(),
            caps: Vec::new(),
        });
    }

    Ok(TracedCharacter {
        parts: traced_parts,
        palette,
        uniform_width: uw,
    })
}

/// Flattens a `BezPath` into polylines of points (closed subpaths implicit).
fn flatten_subpaths(path: &BezPath) -> Vec<Vec<Point>> {
    let mut subpaths: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    flatten(path.iter(), 0.5, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            if current.len() > 1 {
                subpaths.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
            current.push(p);
        }
        kurbo::PathEl::LineTo(p) | kurbo::PathEl::QuadTo(_, p) | kurbo::PathEl::CurveTo(_, _, p) => current.push(p),
        kurbo::PathEl::ClosePath => {
            if current.len() > 1 {
                subpaths.push(std::mem::take(&mut current));
            }
        }
    });
    if current.len() > 1 {
        subpaths.push(current);
    }
    subpaths
}

/// Builds a straight-line `BezPath` through `points` (used to rebuild a
/// stroke after it's been cut at a part boundary).
fn polyline_path(points: &[Point]) -> BezPath {
    let mut path = BezPath::new();
    if let Some(first) = points.first() {
        path.move_to(*first);
        for p in &points[1..] {
            path.line_to(*p);
        }
    }
    path
}

/// Distance from `p` to the segment `a`-`b`.
fn point_segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= f64::EPSILON { 0.0 } else { (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0) };
    ((p.x - (a.x + t * dx)).powi(2) + (p.y - (a.y + t * dy)).powi(2)).sqrt()
}

/// The part (index into `completed.parts`) whose bone segment, head to tail,
/// is nearest the centroid (the mean of the flattened points) of a region that
/// no part owns, and that distance (T11). Only parts with a bone compete; `None` if
/// none has one.
fn nearest_bone_part(
    subpaths: &[Vec<Point>],
    completed: &CompletedParts,
    skel: &Skeleton,
) -> Option<(i32, f64)> {
    let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0usize);
    for p in subpaths.iter().flatten() {
        sx += p.x;
        sy += p.y;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let centroid = Point::new(sx / n as f64, sy / n as f64);
    let mut best: Option<(f64, i32)> = None;
    for (id, part) in completed.parts.iter().enumerate() {
        let Some(bone) = skel.bone(&part.name) else { continue };
        let d = point_segment_distance(
            centroid,
            Point::new(bone.head.0 as f64, bone.head.1 as f64),
            Point::new(bone.tail.0 as f64, bone.tail.1 as f64),
        );
        if best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, id as i32));
        }
    }
    best.map(|(d, id)| (id, d))
}

/// Signed (shoelace) area and the centroid of a closed polygon; a polygon with no
/// area gets the mean of its points as the centroid.
fn polygon_area_centroid(poly: &[Point]) -> (f64, Point) {
    let (mut area2, mut cx, mut cy) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let cross = a.x * b.y - b.x * a.y;
        area2 += cross;
        cx += (a.x + b.x) * cross;
        cy += (a.y + b.y) * cross;
    }
    if area2.abs() < 1e-9 {
        let n = poly.len().max(1) as f64;
        let mean = Point::new(poly.iter().map(|p| p.x).sum::<f64>() / n, poly.iter().map(|p| p.y).sum::<f64>() / n);
        return (0.0, mean);
    }
    (area2 * 0.5, Point::new(cx / (3.0 * area2), cy / (3.0 * area2)))
}

/// `poly` clipped to the rectangle `r` (Sutherland-Hodgman; the winding is kept).
fn clip_polygon_to_rect(poly: &[Point], r: Rect) -> Vec<Point> {
    let mut out = poly.to_vec();
    for edge in 0..4 {
        if out.is_empty() {
            break;
        }
        let input = std::mem::take(&mut out);
        let inside = |p: Point| match edge {
            0 => p.x >= r.x0,
            1 => p.x <= r.x1,
            2 => p.y >= r.y0,
            _ => p.y <= r.y1,
        };
        let cross = |a: Point, b: Point| match edge {
            0 | 1 => {
                let x = if edge == 0 { r.x0 } else { r.x1 };
                Point::new(x, a.y + (x - a.x) / (b.x - a.x) * (b.y - a.y))
            }
            _ => {
                let y = if edge == 2 { r.y0 } else { r.y1 };
                Point::new(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x), y)
            }
        };
        for i in 0..input.len() {
            let (cur, prev) = (input[i], input[(i + input.len() - 1) % input.len()]);
            if inside(cur) {
                if !inside(prev) {
                    out.push(cross(prev, cur));
                }
                out.push(cur);
            } else if inside(prev) {
                out.push(cross(prev, cur));
            }
        }
    }
    out
}

/// Whether a traced shape is part of the drawn eye whose box is `eye_box`: its
/// centroid lies in the box and at least 80 % of its path area lies inside it. A
/// shape with no area (an open stroke) goes by its flattened points instead: their
/// mean and 80 % of them inside the box. The drawn brows (centroid above the box)
/// and everything bigger than the eye fail this and stay where they were traced.
fn belongs_to_drawn_eye(shape: &buzz_scene::ShapeData, eye_box: Rect) -> bool {
    let polys = flatten_subpaths(&shape.path);
    if polys.is_empty() {
        return false;
    }
    let (mut area, mut inside_area, mut cx, mut cy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for poly in &polys {
        let (a, c) = polygon_area_centroid(poly);
        area += a;
        cx += a * c.x;
        cy += a * c.y;
        let clipped = clip_polygon_to_rect(poly, eye_box);
        if clipped.len() >= 3 {
            inside_area += polygon_area_centroid(&clipped).0;
        }
    }
    if shape.fill.is_some() && area.abs() > 1e-6 {
        let centroid = Point::new(cx / area, cy / area);
        return eye_box.contains(centroid) && inside_area / area >= 0.8;
    }
    let points: Vec<Point> = polys.iter().flatten().copied().collect();
    let n = points.len() as f64;
    let mean = Point::new(points.iter().map(|p| p.x).sum::<f64>() / n, points.iter().map(|p| p.y).sum::<f64>() / n);
    let inside = points.iter().filter(|p| eye_box.contains(**p)).count() as f64;
    eye_box.contains(mean) && inside / n >= 0.8
}

/// Owner id at `p` (nearest in-bounds pixel), or `-1` outside the buffer.
fn owner_at(owner: &[i32], w: u32, h: u32, p: Point) -> i32 {
    if w == 0 || h == 0 {
        return -1;
    }
    let x = (p.x.round() as i64).clamp(0, w as i64 - 1) as u32;
    let y = (p.y.round() as i64).clamp(0, h as i64 - 1) as u32;
    owner[(y * w + x) as usize]
}

/// Splits a flattened polyline into contiguous runs sharing the same owner
/// part (item 3: "strokes are cut at the part boundary"), carrying a run's
/// owner across any background gap by nearest-along-the-line fill.
///
/// **No longer used by the trace** (a stroke now goes whole to its majority
/// owner, `trace_character_whole_with`): cutting per pixel run made dashes.
/// Kept for its unit test.
#[allow(dead_code)]
fn cut_stroke_by_owner(points: &[Point], owner: &[i32], w: u32, h: u32) -> Vec<(i32, Vec<Point>)> {
    if points.len() < 2 {
        return Vec::new();
    }
    let mut labels: Vec<i32> = points.iter().map(|p| owner_at(owner, w, h, *p)).collect();
    let mut last = -1i32;
    for l in labels.iter_mut() {
        if *l >= 0 {
            last = *l;
        } else if last >= 0 {
            *l = last;
        }
    }
    let mut next = -1i32;
    for l in labels.iter_mut().rev() {
        if *l >= 0 {
            next = *l;
        } else if next >= 0 {
            *l = next;
        }
    }

    let mut runs: Vec<(i32, Vec<Point>)> = Vec::new();
    let mut cur_label = labels[0];
    let mut cur: Vec<Point> = vec![points[0]];
    for i in 1..points.len() {
        if labels[i] != cur_label {
            cur.push(points[i]);
            runs.push((cur_label, std::mem::take(&mut cur)));
            cur.push(points[i]);
            cur_label = labels[i];
        } else {
            cur.push(points[i]);
        }
    }
    if cur.len() > 1 {
        runs.push((cur_label, cur));
    }
    runs.into_iter().filter(|(label, pts)| *label >= 0 && pts.len() >= 2).collect()
}

/// Approximate polygon-fill area-by-owner histogram (pixel counts), via an
/// even-odd scanline fill restricted to the shape's own bounding box.
fn area_by_owner(subpaths: &[Vec<Point>], owner: &[i32], w: u32, h: u32) -> (HashMap<i32, usize>, usize) {
    let mut hist: HashMap<i32, usize> = HashMap::new();
    if subpaths.is_empty() || w == 0 || h == 0 {
        return (hist, 0);
    }
    let min_y = subpaths.iter().flatten().map(|p| p.y).fold(f64::MAX, f64::min).floor().max(0.0) as i32;
    let max_y = subpaths
        .iter()
        .flatten()
        .map(|p| p.y)
        .fold(f64::MIN, f64::max)
        .ceil()
        .min(h as f64 - 1.0) as i32;
    let mut total = 0usize;
    for y in min_y..=max_y {
        if y < 0 || y >= h as i32 {
            continue;
        }
        let yc = y as f64 + 0.5;
        let mut xs: Vec<f64> = Vec::new();
        for sp in subpaths {
            let n = sp.len();
            if n < 2 {
                continue;
            }
            for i in 0..n {
                let a = sp[i];
                let b = sp[(i + 1) % n];
                if (a.y <= yc && b.y > yc) || (b.y <= yc && a.y > yc) {
                    let t = (yc - a.y) / (b.y - a.y);
                    xs.push(a.x + t * (b.x - a.x));
                }
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut i = 0;
        while i + 1 < xs.len() {
            let x0 = (xs[i].round() as i32).max(0);
            let x1 = (xs[i + 1].round() as i32).min(w as i32);
            for x in x0..x1 {
                let idx = (y as u32 * w + x as u32) as usize;
                *hist.entry(owner[idx]).or_insert(0) += 1;
                total += 1;
            }
            i += 2;
        }
    }
    (hist, total)
}

/// I5: an ink shape whose mean width (`2 · area / perimeter`, px) is under this is
/// line work (the outline network, with whatever masses hang off it) and goes to
/// `TracedPart::ink` however little of it lies on the thin-line mask. A pupil
/// (≈ 9 px) or a brow (≈ 10 px) is wider and stays a mass.
const INK_NETWORK_MAX_MEAN_WIDTH: f64 = 5.0;

/// I5: a shape whose bounds lie inside a face box (an eye, brow or mouth landmark
/// box, see [`protected_face_boxes`], grown by this fraction of its size plus
/// [`FACE_WHOLE_BOX_PAD_PX`]) is a face feature and goes to ONE part whole: the
/// clipped pupil (its eye-part piece plus its head piece) showed the under-painted
/// skin through the seam between the pieces.
const FACE_WHOLE_BOX_GROW: f64 = 0.25;
const FACE_WHOLE_BOX_PAD_PX: f64 = 4.0;
/// ...and that part is the face-feature part (eye, brow, mouth) holding most of it
/// when that holds at least this share of its owned pixels, else the top owner.
const FACE_PART_MIN_SHARE: f64 = 0.25;

/// I5: how far (px) a part's ink piece reaches past its own boundary into its
/// neighbours' (the colour path): a 3 px line along a boundary is drawn whole by
/// both; on rotation a 3 px stub of the neighbour's outline rides along, unseen.
const INK_PIECE_OVERLAP_PX: f64 = 3.0;
/// Distance from `p` to the segment `a`–`b`.
fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    let t = if len2 <= 1e-12 { 0.0 } else { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) };
    (p - (a + ab * t)).hypot()
}

/// I5: splits a region's `remainder` (what lies outside every owner's exact piece)
/// into its outer loops, each with the holes inside it, and gives each to the
/// `qualifying` owner whose bone segment is nearest the loop's centroid (`largest`
/// when no owner has a bone). Returned paths are one per owner, in owner order,
/// so the caller merges each into that owner's piece.
fn split_remainder_by_bone(
    remainder: &BezPath,
    qualifying: &[i32],
    largest: i32,
    completed: &CompletedParts,
    skel: &Skeleton,
) -> Vec<(i32, BezPath)> {
    if remainder.is_empty() {
        return Vec::new();
    }
    let subs = split_subpaths(remainder);
    let pts: Vec<Vec<Point>> = subs.iter().map(|s| flatten_subpaths(s).into_iter().flatten().collect()).collect();
    // A loop whose first point lies inside another loop is a hole of it.
    let is_hole: Vec<bool> = (0..subs.len())
        .map(|i| pts[i].first().is_some_and(|p| (0..subs.len()).any(|j| j != i && subs[j].contains(*p))))
        .collect();
    let bones: Vec<(i32, Point, Point)> = qualifying
        .iter()
        .filter_map(|&id| {
            skel.bone(&completed.parts[id as usize].name).map(|b| {
                (id, Point::new(b.head.0 as f64, b.head.1 as f64), Point::new(b.tail.0 as f64, b.tail.1 as f64))
            })
        })
        .collect();
    let mut groups: Vec<(i32, BezPath)> = Vec::new();
    for i in 0..subs.len() {
        if is_hole[i] || pts[i].len() < 3 {
            continue;
        }
        let (_, centroid) = polygon_area_centroid(&pts[i]);
        let owner = bones
            .iter()
            .map(|(id, a, b)| (segment_distance(centroid, *a, *b), *id))
            .min_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)))
            .map(|(_, id)| id)
            .unwrap_or(largest);
        let group = match groups.iter_mut().find(|(id, _)| *id == owner) {
            Some(g) => &mut g.1,
            None => {
                groups.push((owner, BezPath::new()));
                &mut groups.last_mut().unwrap().1
            }
        };
        group.extend(subs[i].elements().iter().copied());
        for j in 0..subs.len() {
            if is_hole[j] && pts[j].first().is_some_and(|p| subs[i].contains(*p)) {
                group.extend(subs[j].elements().iter().copied());
            }
        }
    }
    groups.sort_by_key(|(id, _)| *id);
    groups
}

/// `2 · area / perimeter`: the mean width of a filled shape (0 for an empty one).
fn mean_width(path: &BezPath) -> f64 {
    let perimeter = path.perimeter(0.1);
    if perimeter <= 1e-9 {
        return 0.0;
    }
    2.0 * path.area().abs() / perimeter
}

/// Whether a part is a face-feature part: an eye, a brow or the mouth.
fn is_face_feature_part(name: &str) -> bool {
    is_eye_part(name) || is_brow_part(name) || name.to_lowercase().contains("mouth")
}

/// I5: the fraction of `path`'s pixel area (the even-odd scanline count of
/// [`area_by_owner`]) whose `line_label` is 1 (on the line mask). 0 for an empty shape.
fn line_share(path: &BezPath, line_label: &[i32], w: u32, h: u32) -> f64 {
    let (hist, total) = area_by_owner(&flatten_subpaths(path), line_label, w, h);
    if total == 0 {
        0.0
    } else {
        hist.get(&1).copied().unwrap_or(0) as f64 / total as f64
    }
}

/// Points on a joint cap's outline: `steps + 1` points from angle `start`
/// sweeping `sweep` radians (a full turn is a closed disc), each at `radius`
/// from `joint` or, if the character's silhouette ends sooner along that ray, at
/// the silhouette. `owner` is the composite's per-pixel owner map (`< 0` is
/// background); the ray is walked in half-pixel steps and stops at the first
/// background pixel, so a cap neither bulges out past a limb nor bridges a gap
/// between two of them.
///
/// `inset` (px) keeps the ring that far **inside** the silhouette and inside
/// `radius`: the silhouette's last owned pixels are the outline's outer half
/// (the matte ends on the ink's anti-alias skirt), so a ring that reached them
/// showed as a sliver of cap colour past the line, and a stroked arc centred on
/// them poked half its width out. The disc uses half a line (1 px); the arc
/// adds half its own stroke width (TRACE_SPILL_AND_OUTPAINT_PLAN, A3b).
fn cap_ring(
    owner: &[i32],
    w: u32,
    h: u32,
    joint: Point,
    radius: f64,
    inset: f64,
    start: f64,
    sweep: f64,
    steps: usize,
) -> Vec<Point> {
    let inset = inset.max(0.0);
    (0..=steps)
        .map(|i| {
            let a = start + sweep * (i as f64 / steps as f64);
            let (c, s) = (a.cos(), a.sin());
            let mut r = 0.0;
            while r + 0.5 + inset <= radius {
                let probe = r + 0.5 + inset;
                let p = Point::new(joint.x + c * probe, joint.y + s * probe);
                if p.x < 0.0 || p.y < 0.0 || p.x >= w as f64 || p.y >= h as f64 || owner_at(owner, w, h, p) < 0 {
                    break;
                }
                r += 0.5;
            }
            Point::new(joint.x + c * r, joint.y + s * r)
        })
        .collect()
}

/// The dominant (non-ink) colour of the pixels `part` itself owns within
/// `radius` of `joint`, or `None` when it owns too few there (fewer than 40) to
/// say. `owner` is the composite's per-pixel owner map and `id` this part's id.
fn local_part_colour(
    part: &CompletedPart,
    owner: &[i32],
    id: i32,
    w: u32,
    h: u32,
    joint: Point,
    radius: f64,
) -> Option<[u8; 3]> {
    let r = radius.ceil() as i64;
    let (cx, cy) = (joint.x.round() as i64, joint.y.round() as i64);
    let side = (2 * r + 1) as u32;
    let mut window = image::RgbaImage::new(side, side);
    let mut mask = vec![0u8; (side * side) as usize];
    let mut count = 0usize;
    for dy in -r..=r {
        for dx in -r..=r {
            let (x, y) = (cx + dx, cy + dy);
            if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 || (dx * dx + dy * dy) as f64 > radius * radius {
                continue;
            }
            let i = (y as u32 * w + x as u32) as usize;
            if owner[i] != id || part.rgba[i * 4 + 3] <= 128 {
                continue;
            }
            let (lx, ly) = ((dx + r) as u32, (dy + r) as u32);
            window.put_pixel(lx, ly, image::Rgba([part.rgba[i * 4], part.rgba[i * 4 + 1], part.rgba[i * 4 + 2], 255]));
            mask[(ly * side + lx) as usize] = 255;
            count += 1;
        }
    }
    (count >= 40).then(|| dominant_color(&window, &mask))
}

/// Radius (px) of a bone's local limb width near its head joint, sampled
/// from the matte's distance transform — the same sampling `complete_parts`
/// uses for its capsule/joint-cap radius.
fn limb_radius_at_head(bone: &Bone, dt: &[f32], w: u32, h: u32, height: f32) -> f32 {
    let (x0, y0) = bone.head;
    let (x1, y1) = bone.tail;
    let (dx, dy) = (x1 - x0, y1 - y0);
    let mut width_sum = 0.0f32;
    let mut width_count = 0;
    for t in [0.1f32, 0.2, 0.3] {
        let sx = (x0 + t * dx).clamp(0.0, (w - 1) as f32) as usize;
        let sy = (y0 + t * dy).clamp(0.0, (h - 1) as f32) as usize;
        let d = dt[sy * w as usize + sx];
        if d > 0.0 {
            width_sum += d;
            width_count += 1;
        }
    }
    let limb_width = if width_count > 0 {
        (width_sum / width_count as f32).max(5.0)
    } else {
        (height * 0.05).max(8.0)
    };
    // Cap at *half* the bone's own length — a cap disc bridges one joint, so
    // its diameter should not exceed the bone it belongs to. A torso bone's
    // width, sampled from the matte's distance transform near a *torso*
    // bone's head (Hips, Chest), reads the distance to the character's outer
    // silhouette from close to the body's centreline, which is nowhere near
    // "half this bone's local width" — it is closer to half the whole
    // character's width; without this cap that balloons the joint cap far
    // past the bone it belongs to and over other parts (a "Hips" cap
    // reaching past the knee) — the root cause of the mis-assigned-fill
    // artefact across pujariFront's kurta hem. A plain `.min(bone.length)`
    // (radius, not diameter, capped at the full length) is not tight enough
    // for a short terminal bone — OldmanfrontWalk's "Foot L" is only ~21px
    // ankle-to-toe, shorter than the foot is wide, so that cap still let a
    // ~42px-wide disc balloon off on its own past the (much smaller) drawn
    // shoe, detached from any artwork it was meant to sit under.
    let bone_len = bone.length.max(1.0);
    //
    // A hand or foot gets a more generous width factor than every other
    // joint (was 0.75 everywhere): it is the *last* bone in its chain, so
    // it alone must bridge the gap that opens between it and its parent
    // (a shin, a forearm) once the two turn independently about two
    // different pivots in a bent pose — the parent draws no cap of its own
    // at that end. Too small a cap there left the stage's white showing
    // through as a pale "ring" around the ankle/wrist. Torso and mid-limb
    // joints keep the tighter factor: they already have this same joint
    // capped from the *other* side too (the child's own head-cap), and a
    // wider factor on Hips/Chest is what the bone-length cap above exists
    // to guard against, not a substitute for it.
    let width_factor = {
        let n = bone.name.to_lowercase();
        if n.contains("foot") || n.contains("hand") || n.contains("wrist") || n.contains("ankle") { 1.3 } else { 0.75 }
    };
    (limb_width * width_factor).min(bone_len * 0.5)
}

fn nearest_palette_color(c: [u8; 3], palette: &[peniko::Color]) -> peniko::Color {
    let mut best = palette[0];
    let mut min_diff = f32::MAX;
    for p in palette {
        let pc = p.to_rgba8();
        let d = (c[0] as f32 - pc.r as f32).powi(2) + (c[1] as f32 - pc.g as f32).powi(2) + (c[2] as f32 - pc.b as f32).powi(2);
        if d < min_diff {
            min_diff = d;
            best = *p;
        }
    }
    best
}

/// How large a filled shape resolved to (near) the line colour must be
/// before `trace_character_whole` treats it as suspect rather than an
/// ordinary small ink accent (a button, a pocket stitch, a wrinkle) that a
/// cartoon's linework legitimately fills dark. See `region_is_dark`.
const DARK_FILL_AREA_GUARD_PX: usize = 150;

/// A traced region with at least this share of its area inside one part goes to
/// that part whole, unclipped (`trace_character_whole_with`).
const WHOLE_REGION_SHARE: f64 = 0.92;

/// A traced region smaller than this (px², at source resolution) always goes to
/// its top part whole: a seam through it would be bigger than the region.
const WHOLE_REGION_SMALL_PX: usize = 400;

/// A straddling region is clipped against every part holding at least this share
/// of the pixels some part owns in it (T9c: 1 %, not 8 % — every part with a real
/// piece of it gets its own piece, so the remainder owner is not handed their
/// pixels; T9d: no pixel floor on top, which kept a 197 px owner of a 633 px region
/// from getting its piece).
const CLIP_MIN_SHARE: f64 = 0.01;

/// Luminance tolerance, in 0..=255, for "close enough to the line colour to
/// be treated as line-coloured" — a quantized fill rarely lands on the line
/// colour exactly.
const DARK_FILL_LUM_TOLERANCE: f32 = 18.0;

/// A luminance below this, averaged over a region, counts as genuinely dark
/// cloth/skin — as opposed to a lightly-quantized fill that merely happens to
/// land near the line colour. Well above `DARK_FILL_LUM_TOLERANCE`: a real
/// dark region still mixes in lighter edge/antialiasing pixels.
const SOURCE_DARK_LUM_THRESHOLD: f32 = 70.0;

fn luminance(c: [u8; 3]) -> f32 {
    0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32
}

/// Whether `fill` is dark enough to read as the line colour rather than a
/// cloth or skin tone.
fn is_near_line_colour(fill: peniko::Color, line_color: peniko::Color) -> bool {
    let f = fill.to_rgba8();
    let l = line_color.to_rgba8();
    luminance([f.r, f.g, f.b]) <= luminance([l.r, l.g, l.b]) + DARK_FILL_LUM_TOLERANCE
}

/// A flat-colour region smaller than this fraction of the picture's pixels is a
/// speck of pattern, not a shape worth drawing: ~390 px (about 20 x 20) on a
/// 1335 x 1178 picture, the same share on any other. See
/// [`clean_composite_regions`].
const SPECK_AREA_FRACTION: f64 = 2.5e-4;

/// The same limit in the **protected** zone (the head and its features), where
/// small coloured marks are the drawing (a smile line, a nostril, a tooth): only
/// what the tracer itself would throw away as dither, ~16 px on a 1335 x 1178
/// picture.
const PROTECTED_SPECK_AREA_FRACTION: f64 = 1.0e-5;

/// A dark (ink-coloured) region whose box is at least this fraction of the
/// picture's diagonal long is a line, however little area it has — an outline, a
/// moustache, an eyebrow, a tooth gap — and is kept even when it is under
/// [`SPECK_AREA_FRACTION`]. ~21 px on a 1335 x 1178 picture.
const INK_LINE_LENGTH_FRACTION: f64 = 0.012;

/// How much lighter than the darkest palette colour (the line colour), in
/// luminance 0..=255, a colour may be and still count as ink for
/// [`clean_composite_regions`].
const INK_LUM_MARGIN: f32 = 40.0;

/// The radius of the window that smooths thin coloured threads away, as a
/// fraction of the picture's longer side (4 px, a 9 px window, on a 1335 px
/// picture), clamped to 2..=8 px. A woven thread is a few pixels wide on any
/// picture this size and the window erases lines up to about its radius wide; much
/// wider and it starts to flip and merge the bands of a striped cloth (a stripe's
/// width is near the window's), which must stay readable.
const THREAD_RADIUS_FRACTION: f64 = 0.003;

/// **Make the composite trace as clean shapes, not speckle**, in place, before
/// the whole-character trace sees it. Returns how many pixels changed palette
/// colour.
///
/// `ColourLines` draws a region for every patch of one colour and an outline
/// stroke along every contrast edge, *including* around the tiny patches it
/// drops as specks. On a patterned cloth (a checked shirt, a printed sari) the
/// threads quantise into hundreds of such patches, and each comes out as a few
/// dark outline dots and dashes: speckle noise across the garment. So, in three
/// steps over each pixel's nearest palette colour:
///
/// 1. **Thin coloured threads are smoothed away.** A non-ink pixel takes the most
///    common non-ink colour in a small window (see [`THREAD_RADIUS_FRACTION`]),
///    which erases lines a few pixels wide and leaves every larger area alone.
///    Ink pixels neither change nor vote.
/// 2. **Every region under [`SPECK_AREA_FRACTION`] of the picture is merged into
///    the neighbouring region it shares the most border with** (preferring
///    neighbours that are themselves big enough to stay), repeating while that
///    leaves new small ones, at most three times.
///
/// **The protected zone** (`protect`: the head, whose thin skin-coloured lines
/// are the smile, nose and ears, and the face features) skips step 1 and merges
/// only what the tracer would throw away as dither anyway
/// ([`PROTECTED_SPECK_AREA_FRACTION`]); left as it is, such a dropped speck gets
/// a ring of outline drawn round it.
/// 3. **The picture is posterised to the palette**, so the tracer, which
///    quantises on its own, sees exactly these regions: no antialiasing fringe
///    or dither left to become dropped specks with a ring of outline round them.
///
/// **Real fine lines stay.** A region of an ink-like colour (within
/// [`INK_LUM_MARGIN`] of `ink` in luminance) whose box is at least
/// [`INK_LINE_LENGTH_FRACTION`] of the picture's diagonal long is a line, not a
/// speck, and is never merged however thin it is; fine ink such as a moustache or
/// a brow is far above the area limit as well. Transparent pixels are never
/// touched or counted as neighbours.
fn clean_composite_regions(
    rgba: &mut [u8],
    w: u32,
    h: u32,
    palette: &[peniko::Color],
    ink: peniko::Color,
    protect: &[bool],
) -> usize {
    let (w, h) = (w as usize, h as usize);
    let n = w * h;
    if n == 0 || palette.is_empty() || rgba.len() < n * 4 || protect.len() < n {
        return 0;
    }
    let min_area = ((n as f64 * SPECK_AREA_FRACTION).round() as usize).max(8);
    let protected_min_area = ((n as f64 * PROTECTED_SPECK_AREA_FRACTION).round() as usize).max(8);
    let min_line = (((w * w + h * h) as f64).sqrt() * INK_LINE_LENGTH_FRACTION).max(8.0);
    let radius = ((w.max(h) as f64 * THREAD_RADIUS_FRACTION).round() as i64).clamp(2, 8);

    let pal: Vec<[i32; 3]> = palette
        .iter()
        .map(|c| {
            let c = c.to_rgba8();
            [c.r as i32, c.g as i32, c.b as i32]
        })
        .collect();
    let ink_lum = {
        let c = ink.to_rgba8();
        luminance([c.r, c.g, c.b]) + INK_LUM_MARGIN
    };
    let is_ink: Vec<bool> = palette
        .iter()
        .map(|c| {
            let c = c.to_rgba8();
            luminance([c.r, c.g, c.b]) <= ink_lum
        })
        .collect();

    // Each opaque pixel's nearest palette colour.
    let mut label = vec![-1i16; n];
    for i in 0..n {
        if rgba[i * 4 + 3] <= 128 {
            continue;
        }
        let (r, g, b) = (rgba[i * 4] as i32, rgba[i * 4 + 1] as i32, rgba[i * 4 + 2] as i32);
        let mut best = 0usize;
        let mut best_d = i32::MAX;
        for (k, p) in pal.iter().enumerate() {
            let d = (r - p[0]).pow(2) + (g - p[1]).pow(2) + (b - p[2]).pow(2);
            if d < best_d {
                best_d = d;
                best = k;
            }
        }
        label[i] = best as i16;
    }
    let original = label.clone();

    // 1. Thin coloured threads: the most common non-ink colour in the window.
    {
        let before = label.clone();
        let mut counts = vec![0u32; palette.len()];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let own = before[i];
                if own < 0 || is_ink[own as usize] || protect[i] {
                    continue;
                }
                counts.iter_mut().for_each(|c| *c = 0);
                let (y0, y1) = ((y as i64 - radius).max(0) as usize, (y as i64 + radius).min(h as i64 - 1) as usize);
                let (x0, x1) = ((x as i64 - radius).max(0) as usize, (x as i64 + radius).min(w as i64 - 1) as usize);
                for ny in y0..=y1 {
                    for &l in &before[ny * w + x0..=ny * w + x1] {
                        if l >= 0 && !is_ink[l as usize] {
                            counts[l as usize] += 1;
                        }
                    }
                }
                let mut best = own as usize;
                for (k, &c) in counts.iter().enumerate() {
                    if c > counts[best] {
                        best = k;
                    }
                }
                label[i] = best as i16;
            }
        }
    }

    // 2. Small regions: merge each into its longest-bordering neighbour.
    struct Patch {
        start: usize,
        len: usize,
        label: i16,
        extent: f64,
        protected: bool,
    }

    for _pass in 0..3 {
        // The four-connected patches of one label.
        let mut patch_of = vec![u32::MAX; n];
        let mut order: Vec<usize> = Vec::with_capacity(n);
        let mut patches: Vec<Patch> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for s in 0..n {
            if label[s] < 0 || patch_of[s] != u32::MAX {
                continue;
            }
            let id = patches.len() as u32;
            let start = order.len();
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
            let mut protected = false;
            patch_of[s] = id;
            stack.push(s);
            while let Some(at) = stack.pop() {
                order.push(at);
                protected |= protect[at];
                let (x, y) = (at % w, at / w);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                let mut visit = |nb: usize| {
                    if label[nb] == label[s] && patch_of[nb] == u32::MAX {
                        patch_of[nb] = id;
                        stack.push(nb);
                    }
                };
                if x > 0 {
                    visit(at - 1);
                }
                if x + 1 < w {
                    visit(at + 1);
                }
                if y > 0 {
                    visit(at - w);
                }
                if y + 1 < h {
                    visit(at + w);
                }
            }
            let (bw, bh) = ((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64);
            patches.push(Patch { start, len: order.len() - start, label: label[s], extent: bw.hypot(bh), protected });
        }

        // Where each small patch goes: (patch, new label).
        let mut moves: Vec<(usize, i16)> = Vec::new();
        for (pi, patch) in patches.iter().enumerate() {
            let limit = if patch.protected { protected_min_area } else { min_area };
            if patch.len >= limit || (is_ink[patch.label as usize] && patch.extent >= min_line) {
                continue;
            }
            // Border contact with each neighbouring label, kept apart for
            // neighbours that are big enough to stay: (label, big, pixels).
            let mut border: Vec<(i16, bool, usize)> = Vec::new();
            for &p in &order[patch.start..patch.start + patch.len] {
                let (x, y) = (p % w, p / w);
                let mut look = |nb: usize| {
                    let l = label[nb];
                    if l < 0 || l == patch.label {
                        return;
                    }
                    let big = patches[patch_of[nb] as usize].len >= limit;
                    match border.iter_mut().find(|e| e.0 == l && e.1 == big) {
                        Some(e) => e.2 += 1,
                        None => border.push((l, big, 1)),
                    }
                };
                if x > 0 {
                    look(p - 1);
                }
                if x + 1 < w {
                    look(p + 1);
                }
                if y > 0 {
                    look(p - w);
                }
                if y + 1 < h {
                    look(p + w);
                }
            }
            let any_big = border.iter().any(|e| e.1);
            if let Some(&(l, _, _)) = border.iter().filter(|e| e.1 == any_big).max_by_key(|e| e.2) {
                moves.push((pi, l));
            }
        }
        if moves.is_empty() {
            break;
        }
        for (pi, new_label) in moves {
            let patch = &patches[pi];
            for &p in &order[patch.start..patch.start + patch.len] {
                label[p] = new_label;
            }
        }
    }

    // 3. Posterise to the palette.
    let mut changed = 0usize;
    for i in 0..n {
        let l = label[i];
        if l < 0 {
            continue;
        }
        if l != original[i] {
            changed += 1;
        }
        let c = pal[l as usize];
        rgba[i * 4] = c[0] as u8;
        rgba[i * 4 + 1] = c[1] as u8;
        rgba[i * 4 + 2] = c[2] as u8;
    }
    changed
}

/// Whether this part's own completed pixels are genuinely dark under `bbox`
/// — the ground truth a suspiciously dark `ColourLines` fill is checked
/// against, since the whole-character trace and this part's own raster are
/// two different readings of the same artwork and only one of them should be
/// believed when they disagree.
/// Is the source **under this shape** dark?
///
/// Measured over the pixels inside the shape itself, not its bounding box:
/// a hair fill's box also covers the forehead and the face beside it, so the
/// box's mean came out skin-bright, the hair was judged "not really dark", and
/// it was recoloured to the part's dominant colour — the washed-orange hair
/// on Guddu's front views. Every other pixel in each direction is sampled; the
/// mean of a solid fill does not need all of them.
fn region_is_dark(part_rgba: &[u8], w: u32, h: u32, path: &kurbo::BezPath) -> bool {
    use kurbo::Shape as _;
    let bbox = path.bounding_box();
    let x0 = bbox.x0.floor().max(0.0) as u32;
    let x1 = (bbox.x1.ceil() as i64).clamp(0, w as i64) as u32;
    let y0 = bbox.y0.floor().max(0.0) as u32;
    let y1 = (bbox.y1.ceil() as i64).clamp(0, h as i64) as u32;
    if x0 >= x1 || y0 >= y1 {
        return false;
    }
    let (mut sum, mut count) = (0.0f64, 0u32);
    for py in (y0..y1.min(h)).step_by(2) {
        for px in (x0..x1.min(w)).step_by(2) {
            if !path.contains(kurbo::Point::new(px as f64 + 0.5, py as f64 + 0.5)) {
                continue;
            }
            let idx = ((py * w + px) as usize) * 4;
            if part_rgba[idx + 3] > 128 {
                sum += luminance([part_rgba[idx], part_rgba[idx + 1], part_rgba[idx + 2]]) as f64;
                count += 1;
            }
        }
    }
    count > 0 && (sum / count as f64) <= SOURCE_DARK_LUM_THRESHOLD as f64
}

/// How [`trace_character_whole_with`] traces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceCharacterOptions {
    /// Stroke width (px) of the joint caps' arcs and, with `outline_strokes`
    /// on, of the boundary strokes.
    pub uniform_width: f64,
    /// Add a uniform-width stroke along every colour boundary and the
    /// silhouette (`TraceMode::ColourLines`). Off by default: on inked art the
    /// ink is already a traced region, and the strokes double it.
    pub outline_strokes: bool,
    /// I5: trace the visible picture ONCE with `TraceMode::InkAndColour`
    /// (`TraceOptions::ink_colour()`: flat colour fills under the ink, colour
    /// lines, smooth filled ink shapes) instead of the T9e pair of traces
    /// (under-painted Outline fills + LineArt brush ink). Only an ink shape
    /// lying mostly on the drawn lines (`ink_line_share`) goes to
    /// `TracedPart::ink`; masses stay fills. `false` keeps the T9e path exactly.
    /// No effect with `outline_strokes` on (that path has no ink layer).
    pub ink_colour: bool,
    /// The trace settings for the `ink_colour` trace (bold, colours, tuning); mode and drop-background are forced.
    pub trace: buzz_scene::trace::TraceOptions,
    /// Share of an ink shape that must lie on the drawn lines for it to be ink; higher keeps more shapes as fills.
    // I5: the share of an `InkAndColour` ink shape's pixel area that must lie on the
    // drawn lines (`InkComponents::line_mask`) for it to be **ink** in
    // [`trace_character_whole_with`] (cut once at the part boundaries, drawn after the
    // fills). Below it the shape is a mass (pupil, brow, hair, shoe) and is tiled with
    // the fills. 60 %: a traced line is a little wider than its line-mask pixels
    // (bold, anti-aliased edge), a mass is mostly `mass_mask`.
    pub ink_line_share: f64,
    /// An ink shape spanning more than this fraction of the figure's height is outline; higher needs a bigger span.
    // I5: an ink shape whose bounds span more than this fraction of the character's
    // height (either way) is the outline network too, however wide its mean.
    pub ink_network_min_span: f64,
    /// How far (fraction of the figure's height) a part's hidden fill reaches past what it owns; higher reaches further.
    // I5: a part's hidden completion reaches at most this fraction of the character's
    // height (clamped to 8–40 px) past the pixels it owns — the strip under a joint,
    // never a whole neighbouring limb the segmenter's mask happened to include.
    pub hidden_reach_fraction: f64,
}

impl Default for TraceCharacterOptions {
    fn default() -> Self {
        Self {
            uniform_width: 2.0,
            outline_strokes: false,
            ink_colour: true,
            trace: buzz_scene::trace::TraceOptions::ink_colour(),
            ink_line_share: 0.60,
            ink_network_min_span: 0.5,
            hidden_reach_fraction: 0.02,
        }
    }
}

/// Composites `completed`'s parts by depth: the straight RGBA picture, and a
/// per-pixel owner id (index into `completed.parts` of the topmost part with
/// alpha above 128, `-1` for background).
fn composite_completed(completed: &CompletedParts) -> (Vec<u8>, Vec<i32>) {
    let (w, h) = (completed.width, completed.height);
    let part_ids: HashMap<String, usize> =
        completed.parts.iter().enumerate().map(|(i, p)| (p.name.clone(), i)).collect();
    let mut ordered: Vec<&CompletedPart> = completed.parts.iter().collect();
    ordered.sort_by_key(|p| p.depth);
    let mut composite_rgba = vec![0u8; (w * h * 4) as usize];
    let mut owner = vec![-1i32; (w * h) as usize];
    for part in &ordered {
        let id = part_ids[&part.name] as i32;
        for i in 0..(w * h) as usize {
            if part.rgba[i * 4 + 3] > 128 {
                composite_rgba[i * 4] = part.rgba[i * 4];
                composite_rgba[i * 4 + 1] = part.rgba[i * 4 + 1];
                composite_rgba[i * 4 + 2] = part.rgba[i * 4 + 2];
                composite_rgba[i * 4 + 3] = part.rgba[i * 4 + 3];
                owner[i] = id;
            }
        }
    }
    (composite_rgba, owner)
}

/// Splits a path into one `BezPath` per subpath (each `MoveTo` starts one).
fn split_subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    let mut cur = BezPath::new();
    for el in path.elements() {
        if matches!(el, kurbo::PathEl::MoveTo(_)) && !cur.elements().is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(*el);
    }
    if cur.elements().len() > 1 {
        out.push(cur);
    }
    out
}

/// Below this mean thickness (`2·|area| / perimeter`, in picture px) a subpath of a
/// clipped piece is a sliver the boolean left behind, not artwork.
const SLIVER_THICKNESS_PX: f64 = 0.25;

/// `path` without the subpaths that enclose (next to) nothing: mean thickness
/// `2·|area| / perimeter` under [`SLIVER_THICKNESS_PX`] (A3 of
/// docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md).
///
/// **Why.** Clipping a region at the part boundaries (`buzz_geom::boolean` where two
/// outlines nearly coincide) leaves zero-area subpaths in the piece: three points a few
/// millionths of a pixel apart, or a line walked out and back. Raju's "Hand R" skin
/// piece carried 32 of them beside its one outline, Guddu's "Shoulder L" cream piece 66
/// beside its outline and its hole. As a fill they paint nothing, but the renderer's
/// seam seal strokes every subpath, and the stroke of one whose points are a single
/// point in f32 leaked along its tile row: a 96 × 14 px skin-coloured band under
/// Raju's hand, a 10 × 12 px block beside Guddu's sleeve. A sliver a few pixels long is
/// sealed as a hairline the fill never drew. Real artwork is far thicker (the thinnest
/// real subpath of those two pieces is 8.7 px; the thickest sliver 0.1 px).
fn without_sliver_subpaths(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    for sub in split_subpaths(path) {
        let length = sub.perimeter(1e-3);
        if length > 0.0 && 2.0 * sub.area().abs() / length >= SLIVER_THICKNESS_PX {
            out.extend(sub);
        }
    }
    out
}

/// Ramer-Douglas-Peucker on an open polyline, keeping both ends.
fn rdp_open(pts: &[Point], tol: f64) -> Vec<Point> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let mut worst = (0usize, 0.0f64);
    for (i, p) in pts.iter().enumerate().skip(1).take(pts.len() - 2) {
        let d = if len2 < 1e-12 {
            (*p - a).hypot()
        } else {
            ((p.x - a.x) * dy - (p.y - a.y) * dx).abs() / len2.sqrt()
        };
        if d > worst.1 {
            worst = (i, d);
        }
    }
    if worst.1 <= tol {
        return vec![a, b];
    }
    let mut left = rdp_open(&pts[..=worst.0], tol);
    let right = rdp_open(&pts[worst.0..], tol);
    left.pop();
    left.extend(right);
    left
}

/// Simplifies every (closed) subpath of `path` to a polygon within `tol` px of
/// it: the flattened curve with Ramer-Douglas-Peucker. Returns the new path and
/// the (before, after) vertex counts.
fn simplify_closed_path(path: &BezPath, tol: f64) -> (BezPath, usize, usize) {
    let (mut before, mut after) = (0usize, 0usize);
    let mut out = BezPath::new();
    for mut pts in flatten_subpaths(path) {
        if pts.len() > 1 && (pts[0] - pts[pts.len() - 1]).hypot() < 1e-9 {
            pts.pop();
        }
        before += pts.len();
        if pts.len() < 4 {
            after += pts.len();
            if pts.len() >= 3 {
                out.move_to(pts[0]);
                for p in &pts[1..] {
                    out.line_to(*p);
                }
                out.close_path();
            }
            continue;
        }
        // Split the ring at the vertex farthest from the first, simplify the
        // two halves, and rejoin them.
        let far = (1..pts.len())
            .max_by(|&i, &j| (pts[i] - pts[0]).hypot().total_cmp(&(pts[j] - pts[0]).hypot()))
            .unwrap_or(1);
        let first = rdp_open(&pts[..=far], tol);
        let mut second_in: Vec<Point> = pts[far..].to_vec();
        second_in.push(pts[0]);
        let second = rdp_open(&second_in, tol);
        let mut ring = first;
        ring.pop();
        ring.extend(second);
        ring.pop();
        after += ring.len();
        if ring.len() < 3 {
            continue;
        }
        out.move_to(ring[0]);
        for p in &ring[1..] {
            out.line_to(*p);
        }
        out.close_path();
    }
    (out, before, after)
}

/// [`trace_character_whole_with`] for a caller that has no source picture:
/// the visible picture is rebuilt from the composite of the completed parts,
/// and boundary strokes are off.
pub fn trace_character_whole(
    completed: &CompletedParts,
    skel: &Skeleton,
    matte: &image::GrayImage,
    uniform_width: Option<f64>,
) -> anyhow::Result<TracedCharacter> {
    let (composite_rgba, _) = composite_completed(completed);
    let source = image::RgbaImage::from_raw(completed.width, completed.height, composite_rgba)
        .ok_or_else(|| anyhow::anyhow!("completed parts have inconsistent dimensions"))?;
    trace_character_whole_with(
        &source,
        completed,
        skel,
        matte,
        &TraceCharacterOptions { uniform_width: uniform_width.unwrap_or(2.0), outline_strokes: false, ..Default::default() },
    )
}

/// A copy of `matte` with every enclosed hole filled: the non-matte pixels
/// (`<= 128`) that cannot be reached from the picture border through other
/// non-matte pixels (4-connected) become matte (`255`).
fn fill_matte_holes(matte: &image::GrayImage) -> image::GrayImage {
    let (w, h) = (matte.width() as usize, matte.height() as usize);
    if w == 0 || h == 0 {
        return matte.clone();
    }
    let raw = matte.as_raw();
    let mut reached = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let seed = |i: usize, reached: &mut Vec<bool>, stack: &mut Vec<usize>| {
        if raw[i] <= 128 && !reached[i] {
            reached[i] = true;
            stack.push(i);
        }
    };
    for x in 0..w {
        seed(x, &mut reached, &mut stack);
        seed((h - 1) * w + x, &mut reached, &mut stack);
    }
    for y in 0..h {
        seed(y * w, &mut reached, &mut stack);
        seed(y * w + w - 1, &mut reached, &mut stack);
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        if x > 0 {
            seed(i - 1, &mut reached, &mut stack);
        }
        if x + 1 < w {
            seed(i + 1, &mut reached, &mut stack);
        }
        if y > 0 {
            seed(i - w, &mut reached, &mut stack);
        }
        if y + 1 < h {
            seed(i + w, &mut reached, &mut stack);
        }
    }
    let mut filled = raw.clone();
    for i in 0..w * h {
        if raw[i] <= 128 && !reached[i] {
            filled[i] = 255;
        }
    }
    image::GrayImage::from_raw(w as u32, h as u32, filled).unwrap_or_else(|| matte.clone())
}

/// The CIELAB distance from the line colour beyond which a connected locally-dark
/// line is not ink (T9c; T9d: judged per 8-connected line by the MEDIAN of its
/// pixels, not per pixel): a saturated dark red / green / purple patch (a plaid
/// line, a sari print, a towel) is a fill with its own colour; dark grey, brown and
/// black lines, anti-aliased edges included, are ink.
pub const CHARACTER_INK_MAX_DELTA_E: f64 = 30.0;

/// **The ink layer's trace options** for a `w` x `h` picture (T9b/T9c): the line tracer
/// in `LineArt` / `Brush` mode, ink found by LOCAL contrast (a pixel under 0.62 x
/// the 80th-percentile luma of the opaque pixels in a window of 2.5 x
/// `max_line_radius`, and under luma 150) so the 2-3 px dark-brown outlines of a face,
/// arm or hand are ink while a dark garment's interior is not, within
/// [`CHARACTER_INK_MAX_DELTA_E`] of `line_colour` (the darkest palette colour; `None`
/// skips that test), and STROKES ONLY (`ink_masses: false`: ink fatter than
/// `max_line_radius` stays a fill).
pub fn character_ink_options(
    w: u32,
    h: u32,
    line_colour: Option<peniko::Color>,
) -> buzz_scene::trace::TraceOptions {
    let max_line_radius = (w.max(h) as f64 / 150.0).clamp(4.0, 12.0);
    buzz_scene::trace::TraceOptions {
        mode: buzz_scene::trace::TraceMode::LineArt,
        line_style: buzz_scene::trace::LineStyle::Brush,
        line_smooth: 0.7,
        fill_enclosed: false,
        max_line_radius,
        min_line_length: 6.0,
        ink_detect: buzz_scene::trace::InkDetect::LocalContrast {
            ratio: 0.62,
            max_luma: 150,
            window: (2.5 * max_line_radius).round() as usize | 1,
            max_delta_e_to_line: CHARACTER_INK_MAX_DELTA_E,
        },
        ink_masses: false,
        drop_background: false,
        line_colour,
        ..Default::default()
    }
}

/// **The thin ink of `visible_rgba`** (straight RGBA, `w` x `h`): exactly the pixels
/// the ink layer ([`character_ink_options`] with `line_colour`) skeletonises and the
/// fills are painted under (before the pipeline clears the protected face boxes).
/// `true` is ink. Masses are not in it.
pub fn character_thin_ink(w: u32, h: u32, visible_rgba: &[u8], line_colour: Option<peniko::Color>) -> Vec<bool> {
    buzz_scene::trace::thin_ink_mask(w, h, visible_rgba, &character_ink_options(w, h, line_colour))
}

/// The line colour the pipeline draws the ink in and measures ink-colouredness
/// against: the darkest colour of the character palette (24 colours) of `completed`.
pub fn character_line_colour(completed: &CompletedParts) -> peniko::Color {
    quantize_character_palette(completed, 24)
        .first()
        .copied()
        .unwrap_or(peniko::Color::from_rgb8(15, 15, 15))
}

/// `r` grown by `frac` of its size on every side, then widened/heightened about its
/// centre to at least `min_side` px each way.
fn padded_box(r: Rect, frac: f64, min_side: f64) -> Rect {
    let (px, py) = (r.width() * frac, r.height() * frac);
    let mut b = Rect::new(r.x0 - px, r.y0 - py, r.x1 + px, r.y1 + py);
    if b.width() < min_side {
        let c = (b.x0 + b.x1) * 0.5;
        (b.x0, b.x1) = (c - min_side * 0.5, c + min_side * 0.5);
    }
    if b.height() < min_side {
        let c = (b.y0 + b.y1) * 0.5;
        (b.y0, b.y1) = (c - min_side * 0.5, c + min_side * 0.5);
    }
    b
}

/// The box of the confident points (confidence above 0.1) of one eye or brow
/// landmark list; fewer than two: `None`.
fn feature_landmark_box(points: &[(f32, f32, f32)]) -> Option<Rect> {
    let confident: Vec<&(f32, f32, f32)> = points.iter().filter(|p| p.2 > 0.1).collect();
    if confident.len() < 2 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in confident {
        x0 = x0.min(p.0 as f64);
        y0 = y0.min(p.1 as f64);
        x1 = x1.max(p.0 as f64);
        y1 = y1.max(p.1 as f64);
    }
    Some(Rect::new(x0, y0, x1, y1))
}

/// **The protected face-feature boxes** (T9c; since T9d only the FALLBACK for a
/// character with no Eye part, see [`protected_by_parts`]), from the
/// skeleton's landmarks alone: the two eyes (landmark box padded 15 % each side, at least 8 px), the two brows
/// (padded 25 %) and the mouth (`mouth_landmark_box` padded 15 %). Inside them
/// the picture is traced as drawn — fills only, no ink, no under-painting — so a
/// drawn eye, brow or mouth is whole and keeps its colours; the jaw, ears, nose line
/// and hair edge outside stay ink. A feature with too few confident landmarks has no
/// box.
pub fn protected_face_boxes(skel: &Skeleton) -> Vec<Rect> {
    let face = &skel.face;
    let mut boxes = Vec::new();
    for eye in [&face.left_eye, &face.right_eye] {
        boxes.extend(feature_landmark_box(eye).map(|b| padded_box(b, 0.15, 8.0)));
    }
    for brow in [&face.left_brow, &face.right_brow] {
        boxes.extend(feature_landmark_box(brow).map(|b| padded_box(b, 0.25, 8.0)));
    }
    boxes.extend(crate::face::mouth_landmark_box(&face.mouth).map(|b| padded_box(b, 0.15, 8.0)));
    boxes
}

/// Which pixels of a `w` x `h` picture lie inside any of `boxes` (a pixel is in a box
/// when its centre is).
fn box_mask(boxes: &[Rect], w: u32, h: u32) -> Vec<bool> {
    let mut mask = vec![false; (w * h) as usize];
    for b in boxes {
        let x0 = (b.x0 - 0.5).ceil().max(0.0) as u32;
        let x1 = ((b.x1 - 0.5).floor() + 1.0).clamp(0.0, w as f64) as u32;
        let y0 = (b.y0 - 0.5).ceil().max(0.0) as u32;
        let y1 = ((b.y1 - 0.5).floor() + 1.0).clamp(0.0, h as f64) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                mask[(y * w + x) as usize] = true;
            }
        }
    }
    mask
}

/// How far the part-mask protected region is grown past the feature parts' own
/// pixels (T9d), in pixels (8-connected rounds).
const PROTECT_GROW_PX: usize = 4;

/// `mask` (`w` x `h`) grown by `r` rounds of 8-connected dilation (a `(2r + 1)` square
/// structuring element), as two box passes over prefix counts.
fn dilate_square(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    if r == 0 || w == 0 || h == 0 {
        return mask.to_vec();
    }
    let mut across = vec![false; w * h];
    let mut prefix = vec![0u32; w.max(h) + 1];
    for y in 0..h {
        for x in 0..w {
            prefix[x + 1] = prefix[x] + mask[y * w + x] as u32;
        }
        for x in 0..w {
            across[y * w + x] = prefix[(x + r + 1).min(w)] > prefix[x.saturating_sub(r)];
        }
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        for y in 0..h {
            prefix[y + 1] = prefix[y] + across[y * w + x] as u32;
        }
        for y in 0..h {
            out[y * w + x] = prefix[(y + r + 1).min(h)] > prefix[y.saturating_sub(r)];
        }
    }
    out
}

/// Whether a part's name is an EYE part the ink layer must leave alone (T9e): contains
/// "eye" but not "brow" (the "Eyebrow" and "Mouth" parts are the segmenter's hair /
/// temple / jaw blobs, not the drawn brows and mouth; the masses protect those),
/// case-insensitively.
fn is_eye_part(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("eye") && !n.contains("brow")
}

/// Whether a part is the head (its name contains "head", case-insensitively).
fn is_head_part(name: &str) -> bool {
    name.to_lowercase().contains("head")
}

/// How far the head's ink masses are grown to make their protected region (T9e), px.
const PROTECT_MASS_GROW_PX: usize = 2;

/// **The protected eye region from the segmenter's part masks** (T9d, T9e): every pixel
/// the owner map gives to an eye part (Left/Right Eye), grown by [`PROTECT_GROW_PX`].
/// The part is the drawn eye itself, so the whole pupil and white is inside (the
/// landmark boxes covered only half of a drawn eye). `None` when no eye part owns a
/// pixel (the caller falls back to [`protected_face_boxes`]).
fn protected_by_parts(completed: &CompletedParts, owner: &[i32], w: u32, h: u32) -> Option<Vec<bool>> {
    let feature: Vec<bool> = completed.parts.iter().map(|p| is_eye_part(&p.name)).collect();
    let own: Vec<bool> = owner.iter().map(|&o| o >= 0 && feature[o as usize]).collect();
    own.iter().any(|&b| b).then(|| dilate_square(&own, w as usize, h as usize, PROTECT_GROW_PX))
}

/// **The head's ink masses, protected** (T9e): the ink MASS pixels (`mass_mask`: a brow,
/// a pupil, hair) that the owner map gives to the head part, grown by
/// [`PROTECT_MASS_GROW_PX`]. A brow or a lock of hair is a fill drawn as is, never
/// stroked or under-painted away. `None` when the head owns no mass pixel.
fn protected_head_masses(
    completed: &CompletedParts,
    owner: &[i32],
    mass_mask: &[bool],
    w: u32,
    h: u32,
) -> Option<Vec<bool>> {
    // T11: a brow part is the drawn brow itself (`brow_parts_from_masses`), so its
    // mass is protected like the head's.
    let head: Vec<bool> = completed.parts.iter().map(|p| is_head_part(&p.name) || is_brow_part(&p.name)).collect();
    let own: Vec<bool> =
        owner.iter().zip(mass_mask).map(|(&o, &mass)| mass && o >= 0 && head[o as usize]).collect();
    own.iter().any(|&b| b).then(|| dilate_square(&own, w as usize, h as usize, PROTECT_MASS_GROW_PX))
}

/// Whether a part is an eyebrow part (its name contains "brow", case-insensitively).
fn is_brow_part(name: &str) -> bool {
    name.to_lowercase().contains("brow")
}

/// 8-connected components of the `true` pixels of `mask` (`w` x `h`): a label per pixel
/// (`0` = not in the mask, else `1..=n`) and `n`.
fn label_components(mask: &[bool], w: usize, h: usize) -> (Vec<u32>, usize) {
    let mut labels = vec![0u32; w * h];
    let mut n = 0u32;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        n += 1;
        labels[start] = n;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if (dx == 0 && dy == 0) || nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if mask[j] && labels[j] == 0 {
                        labels[j] = n;
                        stack.push(j);
                    }
                }
            }
        }
    }
    (labels, n as usize)
}

/// How far the brow landmark box is padded (T11), as a share of its size on each side.
const BROW_BOX_PAD: f64 = 0.6;

/// The least size (px) of the brow landmark box once padded (T11).
const BROW_BOX_MIN_SIDE: f64 = 12.0;

/// **Where a brow's drawn mass may lie** (T11): the box of the brow's confident
/// landmarks padded by [`BROW_BOX_PAD`] (at least [`BROW_BOX_MIN_SIDE`] each way),
/// kept above the top of the same side's eye landmark box (but never cut up into the
/// brow landmarks themselves). `None` with fewer than two confident brow landmarks.
fn brow_search_box(brow: &[(f32, f32, f32)], eye: &[(f32, f32, f32)]) -> Option<Rect> {
    let landmarks = feature_landmark_box(brow)?;
    let mut b = padded_box(landmarks, BROW_BOX_PAD, BROW_BOX_MIN_SIDE);
    if let Some(eye_box) = feature_landmark_box(eye) {
        b.y1 = b.y1.min(eye_box.y0).max(landmarks.y1);
    }
    (b.x1 > b.x0 && b.y1 > b.y0).then_some(b)
}

/// **The brow parts become the drawn brows** (T11). The segmenter's "Left/Right
/// Eyebrow" part is a patch of forehead and hair round the landmarks (100 x 118 px on
/// Raju), so the Brow bone would turn skin, not the brow. Here, per side, the brow
/// part is the ink MASS components (8-connected, `mass_mask`: the dark compact
/// bars) whose centroid lies in [`brow_search_box`], grown by 1 px: those pixels go to
/// the brow part (they win over the head), every pixel the segmenter gave the brow part
/// goes back to the head part, and a brow with no qualifying mass owns nothing. `owner`
/// is corrected in place. Returns the completed parts with the brow and head pictures
/// brought in line (a brow part's picture is the source under its new pixels, so the
/// hidden-fill and dark-fill logic agree with the owner map), `None` when there is no
/// head part or no brow part to correct (`owner` is then untouched).
fn brow_parts_from_masses(
    completed: &CompletedParts,
    owner: &mut [i32],
    mass_mask: &[bool],
    visible_rgba: &[u8],
    skel: &Skeleton,
) -> Option<CompletedParts> {
    let (w, h) = (completed.width as usize, completed.height as usize);
    let head_id = completed.parts.iter().position(|p| is_head_part(&p.name))?;
    let brows: Vec<(usize, bool)> = completed
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| is_brow_part(&p.name))
        .filter_map(|(i, p)| {
            let n = p.name.to_lowercase();
            if n.contains("left") {
                Some((i, true))
            } else if n.contains("right") {
                Some((i, false))
            } else {
                None
            }
        })
        .collect();
    if brows.is_empty() {
        return None;
    }
    let mut fixed = completed.clone();

    // The segmenter's brow patches go back to the head.
    for i in 0..w * h {
        let o = owner[i];
        if o >= 0 && brows.iter().any(|&(b, _)| b as i32 == o) {
            owner[i] = head_id as i32;
            if fixed.parts[head_id].rgba[i * 4 + 3] <= 128 {
                let old = &completed.parts[o as usize].rgba;
                fixed.parts[head_id].rgba[i * 4..i * 4 + 4].copy_from_slice(&old[i * 4..i * 4 + 4]);
            }
        }
    }

    // Each mass component's centroid.
    let (labels, n) = label_components(mass_mask, w, h);
    let mut sums = vec![(0.0f64, 0.0f64, 0usize); n + 1];
    for (i, &l) in labels.iter().enumerate() {
        if l > 0 {
            let s = &mut sums[l as usize];
            s.0 += (i % w) as f64 + 0.5;
            s.1 += (i / w) as f64 + 0.5;
            s.2 += 1;
        }
    }

    for &(b, left) in &brows {
        let (brow_lm, eye_lm) = if left {
            (&skel.face.left_brow, &skel.face.left_eye)
        } else {
            (&skel.face.right_brow, &skel.face.right_eye)
        };
        let mut mass = vec![false; w * h];
        if let Some(search) = brow_search_box(brow_lm, eye_lm) {
            let keep: Vec<bool> = sums
                .iter()
                .enumerate()
                .map(|(l, s)| l > 0 && s.2 > 0 && search.contains(Point::new(s.0 / s.2 as f64, s.1 / s.2 as f64)))
                .collect();
            for (i, &l) in labels.iter().enumerate() {
                mass[i] = l > 0 && keep[l as usize];
            }
        }
        let grown = dilate_square(&mass, w, h, 1);
        let part = &mut fixed.parts[b];
        part.rgba.iter_mut().for_each(|v| *v = 0);
        let (mut sum, mut count) = ([0u64; 3], 0u64);
        for i in 0..w * h {
            if !grown[i] || visible_rgba[i * 4 + 3] == 0 {
                continue;
            }
            // Only the head's pixels (and ones nobody owns) are the brow's to take: an eye
            // part, the other brow or any limb keeps what it owns.
            let o = owner[i];
            if o != head_id as i32 && o != -1 {
                continue;
            }
            owner[i] = b as i32;
            part.rgba[i * 4..i * 4 + 3].copy_from_slice(&visible_rgba[i * 4..i * 4 + 3]);
            part.rgba[i * 4 + 3] = 255;
            if mass[i] {
                for k in 0..3 {
                    sum[k] += visible_rgba[i * 4 + k] as u64;
                }
                count += 1;
            }
        }
        if count > 0 {
            part.dom_color = [(sum[0] / count) as u8, (sum[1] / count) as u8, (sum[2] / count) as u8];
        }
    }
    Some(fixed)
}

/// A region or ink shape no part owns goes to the part with the nearest bone, but it is
/// dropped when its centroid is farther than this many limb radii (at that bone) from the
/// bone segment and it is not attached to the figure (T11, see
/// [`stray_anchor_mask`]).
const STRAY_DISTANCE_FACTOR: f64 = 1.5;

/// How far (px) a loose blob may sit from an owned pixel and still count as touching it.
const STRAY_TOUCH_PX: usize = 2;

/// **The pixels a stray-shape test counts as attached to the figure** (T11): every
/// owned pixel, and every pixel of an 8-connected blob of unowned figure pixels
/// (`visible_rgba` alpha above 0, no owner) that lies within [`STRAY_TOUCH_PX`] of an
/// owned pixel; the whole mask is grown by 1 px so a traced boundary (which runs on pixel
/// edges) lands on it. A bag, a strap or a watch is drawn touching the figure, so all of
/// it is attached; a mark floating beside the figure is not.
fn stray_anchor_mask(visible_rgba: &[u8], owner: &[i32], w: u32, h: u32) -> Vec<bool> {
    let (w, h) = (w as usize, h as usize);
    let owned: Vec<bool> = owner.iter().map(|&o| o >= 0).collect();
    let near_owned = dilate_square(&owned, w, h, STRAY_TOUCH_PX);
    let loose: Vec<bool> = (0..w * h).map(|i| owner[i] < 0 && visible_rgba[i * 4 + 3] > 0).collect();
    let (labels, n) = label_components(&loose, w, h);
    let mut touches = vec![false; n + 1];
    for i in 0..w * h {
        if labels[i] > 0 && near_owned[i] {
            touches[labels[i] as usize] = true;
        }
    }
    let anchored: Vec<bool> =
        (0..w * h).map(|i| owner[i] >= 0 || (labels[i] > 0 && touches[labels[i] as usize])).collect();
    dilate_square(&anchored, w, h, 1)
}

/// Whether any point of the flattened `subpaths` lies on an `anchor` pixel.
fn touches_anchor(subpaths: &[Vec<Point>], anchor: &[bool], w: u32, h: u32) -> bool {
    subpaths.iter().flatten().any(|p| {
        let x = (p.x.floor() as i64).clamp(0, w as i64 - 1) as usize;
        let y = (p.y.floor() as i64).clamp(0, h as i64 - 1) as usize;
        anchor[y * w as usize + x]
    })
}

/// Fills the even-odd interior of `polys` into `img` (`w` x `h`, straight RGBA over an
/// opaque picture) with `colour`, blended at `alpha` (0..=1); the debug picture writer's
/// rasteriser.
fn debug_fill_polys(img: &mut [u8], w: u32, h: u32, polys: &[Vec<Point>], colour: [u8; 3], alpha: f32) {
    let (mut y0, mut y1) = (f64::MAX, f64::MIN);
    for p in polys.iter().flatten() {
        y0 = y0.min(p.y);
        y1 = y1.max(p.y);
    }
    if y0 > y1 {
        return;
    }
    let (row0, row1) = ((y0.floor().max(0.0)) as u32, (y1.ceil().min(h as f64 - 1.0)).max(0.0) as u32);
    for y in row0..=row1 {
        let yc = y as f64 + 0.5;
        let mut xs: Vec<f64> = Vec::new();
        for poly in polys {
            for k in 0..poly.len() {
                let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
                if (a.y <= yc) != (b.y <= yc) {
                    xs.push(a.x + (yc - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for span in xs.chunks_exact(2) {
            let xa = (span[0] - 0.5).ceil().max(0.0) as u32;
            let xb = ((span[1] - 0.5).floor() + 1.0).clamp(0.0, w as f64) as u32;
            for x in xa..xb {
                let i = (y * w + x) as usize * 4;
                for k in 0..3 {
                    img[i + k] = (img[i + k] as f32 * (1.0 - alpha) + colour[k] as f32 * alpha).round() as u8;
                }
            }
        }
    }
}

/// A distinct colour per part id (golden-angle hue), for the debug picture.
fn debug_part_colour(id: usize) -> [u8; 3] {
    let hue = (id as f32 * 137.508) % 360.0;
    let (s, v) = (0.85f32, 0.9f32);
    let c = v * s;
    let hp = hue / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
}

/// Writes `BUZZ_TRACE_PARTS_PNG` (T11): every part's shapes (fills and ink) in its own
/// colour over white, in part order, then the shapes the stray rule dropped in solid red;
/// prints each part's colour. For review only.
fn write_parts_debug_png(
    path: &std::ffi::OsStr,
    w: u32,
    h: u32,
    completed: &CompletedParts,
    per_part_shapes: &[Vec<buzz_scene::ShapeData>],
    per_part_ink: &[Vec<buzz_scene::ShapeData>],
    per_part_caps: &[Vec<buzz_scene::ShapeData>],
    dropped: &[BezPath],
) {
    let mut img = vec![255u8; (w * h * 4) as usize];
    for (id, part) in completed.parts.iter().enumerate() {
        let colour = debug_part_colour(id);
        let shapes = per_part_caps[id].iter().chain(&per_part_shapes[id]).chain(&per_part_ink[id]);
        let mut count = 0usize;
        for shape in shapes {
            count += 1;
            if shape.fill.is_some() {
                debug_fill_polys(&mut img, w, h, &flatten_subpaths(&shape.path), colour, 0.75);
            }
        }
        eprintln!(
            "trace_character_whole: part {:?} = rgb({}, {}, {}), {count} shapes",
            part.name, colour[0], colour[1], colour[2]
        );
    }
    for path in dropped {
        debug_fill_polys(&mut img, w, h, &flatten_subpaths(path), [255, 0, 0], 1.0);
    }
    match image::RgbaImage::from_raw(w, h, img) {
        Some(out) => {
            if let Err(e) = out.save(path) {
                eprintln!("trace_character_whole: cannot write {}: {e}", std::path::Path::new(path).display());
            }
        }
        None => eprintln!("trace_character_whole: the parts picture has the wrong buffer size"),
    }
}

/// The outline paths (with their bounding boxes) of the `true` pixels of `mask`: the
/// shapes the ink brush strokes are cut away from. Traced like a part boundary
/// (outline mode, lightly smoothed).
fn mask_cut_paths(mask: &[bool], w: u32, h: u32) -> Vec<(Rect, BezPath)> {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (i, &on) in mask.iter().enumerate() {
        if on {
            rgba[i * 4 + 3] = 255;
        }
    }
    let opts = buzz_scene::trace::TraceOptions {
        mode: buzz_scene::trace::TraceMode::Outline,
        smooth: 0.5,
        speckle: 4,
        ..Default::default()
    };
    buzz_scene::trace::trace(w, h, &rgba, &opts)
        .shapes
        .into_iter()
        .filter(|s| s.fill.is_some() && !s.path.is_empty())
        .map(|s| (s.path.bounding_box(), s.path))
        .collect()
}

/// `visible_rgba` with the thin ink and its halo repainted by the colour of the nearest
/// clean pixel (T9d): a multi-source breadth-first search, so the fills traced from
/// the result run *under* the strokes and no dark line-shaped region is left to peek
/// out from beside a slightly thinner stroke.
///
/// The paint set `P` is `ink` grown by 1 px (8-connected; the dark anti-aliased halo
/// is repainted too) minus `protected`. The SEEDS are the present (alpha above 0)
/// pixels outside `P` grown by a further 1 px, outside `protected`: every seed is at
/// least 3 px from ink, never a halo pixel (halo seeds made the nearest seed alternate
/// dark / skin: a diagonal hatch on skin). The search grows in distance layers
/// (8-connected) through the pixels within 2 px of the ink; each takes the colour of
/// the SEED that reached it first (a colour is passed on unchanged, never a blend), and
/// those in `P` are repainted with it (the 1 px ring between `P` and the seeds only
/// carries the colour, it keeps its own). Protected pixels neither donate nor receive
/// (the fills see the original pixels there); transparent pixels never donate, carry
/// or receive; a pixel of `P` no seed can reach keeps its own colour.
fn paint_under_ink(visible_rgba: &[u8], ink: &[bool], protected: &[bool], w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = visible_rgba.to_vec();
    let present = |i: usize| visible_rgba[i * 4 + 3] > 0;
    let halo = dilate_square(ink, w, h, 1);
    let paint: Vec<bool> = (0..w * h).map(|i| halo[i] && !protected[i]).collect();
    let near = dilate_square(&paint, w, h, 1);
    // The colour each visited pixel carries; `seen` marks the seeds and visited pixels.
    let mut carried: Vec<[u8; 3]> =
        (0..w * h).map(|i| [visible_rgba[i * 4], visible_rgba[i * 4 + 1], visible_rgba[i * 4 + 2]]).collect();
    let mut seen: Vec<bool> = (0..w * h).map(|i| !(present(i) && near[i] && !protected[i])).collect();
    let mut frontier: Vec<usize> = (0..w * h).filter(|&i| present(i) && !near[i] && !protected[i]).collect();
    // Only pixels within 2 px of ink (`near`) are ever reached; far seeds just start
    // the search at its edge, so skip the ones with no such neighbour.
    frontier.retain(|&i| {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        (-1i64..=1).any(|dy| {
            (-1i64..=1).any(|dx| {
                let (nx, ny) = (x + dx, y + dy);
                nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64 && near[ny as usize * w + nx as usize]
            })
        })
    });
    while !frontier.is_empty() {
        let mut next: Vec<usize> = Vec::new();
        for &i in &frontier {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let rgb = carried[i];
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if (dx == 0 && dy == 0) || nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if !seen[n] {
                        seen[n] = true;
                        carried[n] = rgb;
                        if paint[n] {
                            out[n * 4..n * 4 + 3].copy_from_slice(&rgb);
                        }
                        next.push(n);
                    }
                }
            }
        }
        frontier = next;
    }
    out
}

/// Traces the **visible source picture in one pass** (never trace parts
/// separately, which stippled faces and washed out colour), then splits the
/// resulting vector shapes by part, and adds a vector joint cap plus a
/// close-the-cut base fill per part.
///
/// The picture traced is `source` where `matte > 128`, untouched: no region
/// cleaning, no posterising, so a plaid, a pen or an eyebrow survives and every
/// fill carries the tracer's exact mean colour for its region. A region lying
/// at least 92% inside one part (or smaller than 400 px) goes to that part
/// whole; one that straddles parts is boolean-clipped against each owner
/// holding 1%+ of it, along that part's smoothed, simplified boundary. With
/// `options.outline_strokes` boundary strokes are traced too (`ColourLines`)
/// and each goes whole to the part holding most of its points.
///
/// `completed` supplies the per-pixel part ownership; `skel` each bone's
/// head/tail for the joint caps; `matte` the local limb width (via its
/// distance transform) the caps and boundary smoothing are sized from.
pub fn trace_character_whole_with(
    source: &image::RgbaImage,
    completed: &CompletedParts,
    skel: &Skeleton,
    matte: &image::GrayImage,
    options: &TraceCharacterOptions,
) -> anyhow::Result<TracedCharacter> {
    let (w, h) = (completed.width, completed.height);
    if source.width() != w || source.height() != h {
        anyhow::bail!(
            "source picture is {}x{} but the completed parts are {w}x{h}",
            source.width(),
            source.height()
        );
    }
    if matte.width() != w || matte.height() != h {
        anyhow::bail!("matte is {}x{} but the completed parts are {w}x{h}", matte.width(), matte.height());
    }
    let palette = quantize_character_palette(completed, 24);
    let line_color = palette.first().copied().unwrap_or(peniko::Color::from_rgb8(15, 15, 15));
    let uw = options.uniform_width;

    // --- The per-pixel owner label map (the topmost part at each pixel).
    let (_, mut owner) = composite_completed(completed);

    // --- The visible picture: the source wherever the matte says
    // "character", transparent elsewhere. Not cleaned, not posterised.
    //
    // **Enclosed holes in the matte are filled first**: a patch the segmenter
    // left out inside the figure (Guddu's hair highlight) is not background,
    // it is part of the picture, and would otherwise be a hole nothing covers.
    let filled_matte = fill_matte_holes(matte);
    let mut visible_rgba = vec![0u8; (w * h * 4) as usize];
    for (i, (px, m)) in source.pixels().zip(filled_matte.pixels()).enumerate() {
        if m[0] > 128 {
            visible_rgba[i * 4..i * 4 + 4].copy_from_slice(&px.0);
        }
    }

    // --- T9e/T11: the ink classified line vs mass (compactness), up front: the brow
    // parts are the drawn brow masses, so the owner map is corrected BEFORE anything
    // reads it. Only with the ink layer on (`outline_strokes` off).
    let ink_options = character_ink_options(w, h, Some(line_color));
    let comps: Option<buzz_scene::trace::InkComponents> = (!options.outline_strokes)
        .then(|| buzz_scene::trace::ink_components(w, h, &visible_rgba, &ink_options));
    // T11: the brow part owns the drawn brow (the dark mass in the brow landmark box),
    // the rest of the segmenter's patch is the head's. From here on `completed` is the
    // corrected picture set and `owner` its owner map.
    let corrected = comps
        .as_ref()
        .and_then(|c| brow_parts_from_masses(completed, &mut owner, &c.mass_mask, &visible_rgba, skel));
    let completed = corrected.as_ref().unwrap_or(completed);
    if corrected.is_some() && std::env::var_os("BUZZ_TRACE_INK_LOG").is_some() {
        for (id, part) in completed.parts.iter().enumerate().filter(|(_, p)| is_brow_part(&p.name)) {
            let px = owner.iter().filter(|&&o| o == id as i32).count();
            eprintln!("trace_character_whole: brow part {:?} owns {px} px (the drawn brow mass)", part.name);
        }
    }

    let dt = crate::segment::distance_transform(matte);
    let height = skel.character_height.max(10.0);

    // Each part's own smoothed boundary (for item 3's boolean clip) and
    // dominant fill colour (for item 4's joint cap and close-the-cut fill).
    let mut part_boundary: Vec<BezPath> = Vec::with_capacity(completed.parts.len());
    let mut part_dom_color: Vec<peniko::Color> = Vec::with_capacity(completed.parts.len());
    for (id, part) in completed.parts.iter().enumerate() {
        let mut owner_mask = vec![0u8; (w * h) as usize];
        let mut sil_buf = vec![0u8; (w * h * 4) as usize];
        let mut has_silhouette = false;
        for i in 0..owner.len() {
            if owner[i] == id as i32 {
                owner_mask[i] = 255;
                sil_buf[i * 4 + 3] = 255;
                has_silhouette = true;
            }
        }
        let boundary = if has_silhouette {
            // The cut line must be smooth, not the owner mask's pixel
            // staircase: trace the mask fully smoothed (dropping owner islands
            // under 64 px), then simplify it to within `max(1.5 px, limb
            // width / 12)` so a long limb keeps a long, clean edge.
            let sil_opts = buzz_scene::trace::TraceOptions {
                mode: buzz_scene::trace::TraceMode::Outline,
                smooth: 1.0,
                speckle: 64,
                ..Default::default()
            };
            let sil_report = buzz_scene::trace::trace(w, h, &sil_buf, &sil_opts);
            let tol = match skel.bone(&part.name) {
                Some(bone) => (limb_radius_at_head(bone, &dt, w, h, height) as f64 * 2.0 / 12.0).max(1.5),
                None => 1.5,
            };
            let (mut verts_before, mut verts_after) = (0usize, 0usize);
            let paths: Vec<BezPath> = sil_report
                .shapes
                .into_iter()
                .filter_map(|s| {
                    let (simplified, before, after) = simplify_closed_path(&s.path, tol);
                    verts_before += before;
                    verts_after += after;
                    (!simplified.elements().is_empty()).then_some(simplified)
                })
                .collect();
            if std::env::var_os("BUZZ_TRACE_BOUNDARY_LOG").is_some() {
                eprintln!(
                    "trace_character_whole: part {:?} boundary {verts_before} -> {verts_after} vertices (tolerance {tol:.2} px)",
                    part.name
                );
            }
            match paths.len() {
                0 => BezPath::new(),
                1 => paths.into_iter().next().unwrap(),
                _ => buzz_geom::union_all(&paths, BooleanOptions::for_shape_size(w.max(h) as f64)),
            }
        } else {
            BezPath::new()
        };
        part_boundary.push(boundary);

        // The joint-cap disc and close-the-cut base fill (item 4) must use
        // this *part's own* dominant fill colour — `part.dom_color`, set by
        // `complete_parts` from the best evidence it had (its own segmented
        // pixels, else a growing local probe, else the whole character; see
        // `CompletedPart::dom_color`'s doc comment). It is deliberately
        // *not* re-derived here from `owner_mask` (only the sliver still
        // visible on top once every other part is composited — empty or a
        // thin, edge-biased sliver for a heavily occluded part, e.g. a foot
        // mostly covered by a trouser leg, or a hip mostly covered by a
        // kurta hem) nor from this part's own completed alpha (which can
        // *also* be empty right where a badly detected joint sits in empty
        // background — the raster completion correctly refuses to paint
        // outside the character's silhouette there). Both were the "pale
        // disc at OldmanfrontWalk's foot" / "stray dark filled band across
        // pujariFront's kurta hem" artefacts.
        part_dom_color.push(nearest_palette_color(part.dom_color, &palette));
    }

    // --- The single whole-character trace of the visible picture. Regions
    // only (their fills are the tracer's exact per-region mean colours); the
    // palette just drives the region flood. Boundary strokes are opt-in.
    let trace_options = if options.outline_strokes {
        buzz_scene::trace::TraceOptions {
            mode: buzz_scene::trace::TraceMode::ColourLines,
            line_style: buzz_scene::trace::LineStyle::Stroke,
            uniform_width: Some(uw),
            line_smooth: 0.6,
            fill_enclosed: true,
            colours: palette.len().max(24),
            line_colour: Some(line_color),
            ..Default::default()
        }
    } else {
        // T9: the ink carries the detail, so the fills may be rounder.
        buzz_scene::trace::TraceOptions {
            mode: buzz_scene::trace::TraceMode::Outline,
            colours: 32,
            smooth: 0.75,
            detail: 1.8,
            ..Default::default()
        }
    };

    // --- T9/T9b: the ink as brush strokes. The line tracer traces the figure's
    // thin ink (locally dark strokes, see `character_ink_options`; masses stay
    // fills) into smooth variable-width brush fills: the outlines. The fills are
    // traced from the picture with the thin-ink pixels repainted by their
    // neighbours' colours, so they run UNDER the strokes. With
    // `outline_strokes` on, the old single trace stands (boundary strokes
    // already draw the lines).
    //
    // I5: with `ink_colour` on (and the ink layer on), none of this: ONE
    // `TraceMode::InkAndColour` trace of the visible picture (it under-paints
    // itself and keeps pupils, brows and glints whole by construction).
    let use_ink_colour = options.ink_colour && !options.outline_strokes;
    let ink_colour_report = use_ink_colour.then(|| {
        let mut trace_options = options.trace;
        trace_options.mode = buzz_scene::trace::TraceMode::InkAndColour;
        trace_options.drop_background = true;
        buzz_scene::trace::trace(w, h, &visible_rgba, &trace_options)
    });
    let (fill_source, ink_report) = if options.outline_strokes || use_ink_colour {
        (std::borrow::Cow::Borrowed(&visible_rgba[..]), None)
    } else {
        // T9e: the ink is classified line vs mass by compactness (`ink_components`):
        // only the LINE components are traced as strokes and under-painted; the masses
        // (brows, pupils, hair, shoes) stay fills exactly as drawn.
        let comps = comps.as_ref().expect("the ink components exist whenever outline_strokes is off");
        // T9c-T9e: the protected region is the segmenter's Eye part masks grown 4 px (the
        // landmark boxes only if the character has no Eye part), plus the ink masses
        // that lie in the head part, grown 2 px. No ink and no under-painting there: the
        // brush shapes are cut away from it and the under-painting leaves it alone, so
        // the fills there are the picture as drawn.
        let (mut protected, mut cut_paths) = match protected_by_parts(completed, &owner, w, h) {
            Some(mask) => {
                let cut = mask_cut_paths(&mask, w, h);
                (mask, cut)
            }
            None => {
                let boxes = protected_face_boxes(skel);
                let cut: Vec<(Rect, BezPath)> = boxes.iter().map(|b| (*b, b.to_path(0.1))).collect();
                (box_mask(&boxes, w, h), cut)
            }
        };
        if let Some(masses) = protected_head_masses(completed, &owner, &comps.mass_mask, w, h) {
            cut_paths.extend(mask_cut_paths(&masses, w, h));
            for (p, &m) in protected.iter_mut().zip(&masses) {
                *p |= m;
            }
        }
        if let Some(path) = std::env::var_os("BUZZ_TRACE_PROTECTED_PNG") {
            // The protected region (white) over the picture (darkened), for review.
            let mut img = image::RgbaImage::new(w, h);
            for (i, out) in img.pixels_mut().enumerate() {
                let g = (255 - visible_rgba[i * 4 + 3] as u32 / 2) as u8;
                *out = if protected[i] {
                    image::Rgba([255, 0, 255, 255])
                } else {
                    image::Rgba([visible_rgba[i * 4] / 2 + g / 4, visible_rgba[i * 4 + 1] / 2 + g / 4, visible_rgba[i * 4 + 2] / 2 + g / 4, 255])
                };
            }
            let _ = img.save(&path);
        }
        let mut ink = comps.line_mask.clone();
        for (i, &p) in protected.iter().enumerate() {
            if p {
                ink[i] = false;
            }
        }
        let under_rgba = paint_under_ink(&visible_rgba, &ink, &protected, w, h);
        if let Some(path) = std::env::var_os("BUZZ_TRACE_UNDER_PNG") {
            // The under-painted picture over white, for review (`rig_vs_source`).
            let mut img = image::RgbaImage::new(w, h);
            for (px, out) in under_rgba.chunks_exact(4).zip(img.pixels_mut()) {
                let a = px[3] as f32 / 255.0;
                let c = |v: u8| (v as f32 * a + 255.0 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
                *out = image::Rgba([c(px[0]), c(px[1]), c(px[2]), 255]);
            }
            if let Err(e) = img.save(&path) {
                eprintln!("trace_character_whole: cannot write {}: {e}", std::path::Path::new(&path).display());
            }
        }
        let mut ink_report = buzz_scene::trace::trace(w, h, &visible_rgba, &ink_options);
        if !cut_paths.is_empty() {
            let bool_opts = BooleanOptions::for_shape_size(w.max(h) as f64);
            ink_report.shapes = std::mem::take(&mut ink_report.shapes)
                .into_iter()
                .filter_map(|mut shape| {
                    for (rect, box_path) in &cut_paths {
                        let bb = shape.path.bounding_box();
                        if bb.x0 < rect.x1 && bb.x1 > rect.x0 && bb.y0 < rect.y1 && bb.y1 > rect.y0 {
                            shape.path = buzz_geom::boolean(&shape.path, box_path, BoolOp::Difference, bool_opts);
                        }
                    }
                    (!shape.path.is_empty()).then_some(shape)
                })
                .collect();
        }
        if std::env::var_os("BUZZ_TRACE_INK_LOG").is_some() {
            let present = visible_rgba.chunks_exact(4).filter(|px| px[3] > 0).count().max(1);
            let ink_px = ink.iter().filter(|&&v| v).count();
            eprintln!(
                "trace_character_whole: local-contrast thin ink {:.1}% of the figure ({ink_px} px), {} ink strokes, {} protected px",
                100.0 * ink_px as f64 / present as f64,
                ink_report.strokes,
                protected.iter().filter(|&&v| v).count()
            );
        }
        (std::borrow::Cow::Owned(under_rgba), Some(ink_report))
    };
    let report = match ink_colour_report {
        Some(r) => r,
        None => buzz_scene::trace::trace(w, h, &fill_source, &trace_options),
    };

    // --- Item 3: split the traced shapes by part ---
    let mut per_part_shapes: Vec<Vec<buzz_scene::ShapeData>> = vec![Vec::new(); completed.parts.len()];
    // T9: the ink shapes go through the same owner logic into their own list.
    let mut per_part_ink: Vec<Vec<buzz_scene::ShapeData>> = vec![Vec::new(); completed.parts.len()];
    let diag = ((w * w + h * h) as f64).sqrt();
    let bool_opts = BooleanOptions::for_shape_size(diag);
    // The part with the most owned pixels, worked out the first time a region
    // no part owns finds a skeleton with no bone for any part.
    let mut most_pixels_part: Option<Option<i32>> = None;
    // T11: the stray rule's attachment mask (built the first time it is needed), how many
    // unowned shapes it dropped, and their paths (for `BUZZ_TRACE_PARTS_PNG`).
    let mut stray_anchor: Option<Vec<bool>> = None;
    let mut stray_dropped = 0usize;
    let mut stray_paths: Vec<BezPath> = Vec::new();
    let stray_log = std::env::var_os("BUZZ_TRACE_INK_LOG").is_some();

    let tagged: Vec<(bool, buzz_scene::ShapeData)> = if use_ink_colour {
        // I5: `shapes[..fills]` are the colour fills, `shapes[fills..fills + strokes]`
        // the colour lines (both tiled as fills, in that order), the rest the ink
        // shapes. An ink shape is ink (cut once at the part boundaries, drawn on top)
        // only when it lies mostly on the drawn LINES (`comps.line_mask`); a mass
        // (pupil, brow, hair, shoe) stays a fill so the face logic, the brow parts
        // and the lip-sync mouth still see it. The tracer's order is kept.
        let comps = comps.as_ref().expect("the ink components exist whenever outline_strokes is off");
        let line_label: Vec<i32> = comps.line_mask.iter().map(|&l| l as i32).collect();
        let n_colour = (report.fills + report.strokes).min(report.shapes.len());
        report
            .shapes
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                // A line NETWORK is ink whatever its share on the thin-line mask:
                // the figure's outline is one 8-connected ink component with the
                // hair and shoe masses hanging off it, so under 60 % of it is
                // "thin", yet tiling it as a fill handed the whole body outline
                // to the head. Its mean width tells it from a pupil or a brow.
                let (share, width) = if i >= n_colour {
                    (line_share(&s.path, &line_label, w, h), mean_width(&s.path))
                } else {
                    (0.0, 0.0)
                };
                // ...and one spanning half the figure is the outline network
                // whatever its mean width (Raju's: 6.6 px, with the hair, shoes,
                // belt and bag strap fused into it); no mass is that big.
                let bb = s.path.bounding_box();
                let spans_figure = bb.width().max(bb.height()) > options.ink_network_min_span * height as f64;
                let is_ink = i >= n_colour
                    && (share >= options.ink_line_share || width < INK_NETWORK_MAX_MEAN_WIDTH || spans_figure);
                if i >= n_colour && std::env::var_os("BUZZ_TRACE_INK_LOG").is_some() {
                    eprintln!(
                        "trace_character_whole: ink shape {}: area {:.0} perimeter {:.0} mean width {width:.1} line share {share:.2} -> {} bbox ({:.0},{:.0})-({:.0},{:.0})",
                        i - n_colour,
                        s.path.area().abs(),
                        s.path.perimeter(0.1),
                        if is_ink { "INK" } else { "mass" },
                        bb.x0,
                        bb.y0,
                        bb.x1,
                        bb.y1
                    );
                }
                (is_ink, s)
            })
            .collect()
    } else {
        let ink_shapes = ink_report.map(|r| r.shapes).unwrap_or_default();
        report
            .shapes
            .into_iter()
            .map(|s| (false, s))
            .chain(ink_shapes.into_iter().map(|s| (true, s)))
            .collect()
    };
    // I5 face rules (the Ink + colour path only): the face boxes a shape goes
    // whole inside, and the face-feature parts that own nothing outside them.
    let face_rules = options.ink_colour && !options.outline_strokes;
    let face_part: Vec<bool> = completed.parts.iter().map(|p| is_face_feature_part(&p.name)).collect();
    // I5: a part's INK piece is clipped by its boundary grown `INK_PIECE_OVERLAP_PX`
    // (the colour path only), so a line running along a boundary is drawn whole
    // by both neighbours and the upper one's fill cannot hide the lower one's
    // piece (the collar line came out dashed).
    let part_ink_clip: Vec<BezPath> = if face_rules {
        part_boundary
            .iter()
            .enumerate()
            .map(|(id, b)| {
                if b.elements().is_empty() {
                    return b.clone();
                }
                let clip = buzz_geom::expand_fill(b, INK_PIECE_OVERLAP_PX, BooleanOptions::for_shape_size(w.max(h) as f64));
                if std::env::var_os("BUZZ_TRACE_INK_LOG").is_some() {
                    let (bb, cb) = (b.bounding_box(), clip.bounding_box());
                    eprintln!(
                        "trace_character_whole: ink clip {:?}: boundary ({:.0},{:.0})-({:.0},{:.0}) {} els -> grown ({:.0},{:.0})-({:.0},{:.0}) {} els",
                        completed.parts[id].name,
                        bb.x0,
                        bb.y0,
                        bb.x1,
                        bb.y1,
                        b.elements().len(),
                        cb.x0,
                        cb.y0,
                        cb.x1,
                        cb.y1,
                        clip.elements().len()
                    );
                }
                clip
            })
            .collect()
    } else {
        Vec::new()
    };
    let face_boxes: Vec<Rect> = if face_rules {
        // The landmark boxes mark the eye's contour points, which on a cartoon
        // stop short of the drawn white (Raju's eye landmarks end 10 px above
        // the white's bottom edge), so each is grown to the extent of the
        // segmenter's face part whose pixels centre in it — the Eye part's
        // mask does cover the drawn eye — before the inflation.
        let mut boxes = protected_face_boxes(skel);
        let mut part_bbox: Vec<Option<Rect>> = vec![None; completed.parts.len()];
        for (i, &o) in owner.iter().enumerate() {
            if o >= 0 && face_part[o as usize] {
                let (x, y) = ((i as u32 % w) as f64, (i as u32 / w) as f64);
                let px = Rect::new(x, y, x + 1.0, y + 1.0);
                part_bbox[o as usize] = Some(match part_bbox[o as usize] {
                    Some(b) => b.union(px),
                    None => px,
                });
            }
        }
        for pb in part_bbox.into_iter().flatten() {
            match boxes.iter_mut().find(|b| b.contains(pb.center())) {
                Some(b) => *b = b.union(pb),
                None => boxes.push(pb),
            }
        }
        boxes
            .into_iter()
            .map(|b| {
                b.inflate(b.width() * FACE_WHOLE_BOX_GROW + FACE_WHOLE_BOX_PAD_PX, b.height() * FACE_WHOLE_BOX_GROW + FACE_WHOLE_BOX_PAD_PX)
            })
            .collect()
    } else {
        Vec::new()
    };
    if std::env::var_os("BUZZ_TRACE_INK_LOG").is_some() {
        for b in &face_boxes {
            eprintln!("trace_character_whole: face box ({:.0},{:.0})-({:.0},{:.0})", b.x0, b.y0, b.x1, b.y1);
        }
    }
    for (is_ink, shape) in tagged {
        if shape.fill.is_some() {
            let subpaths = flatten_subpaths(&shape.path);
            let (hist, total) = area_by_owner(&subpaths, &owner, w, h);
            if total == 0 {
                continue;
            }
            let mut owners: Vec<(i32, usize)> =
                hist.iter().filter(|&(&id, _)| id >= 0).map(|(&id, &n)| (id, n)).collect();
            owners.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let bb = shape.path.bounding_box();
            let in_face_box =
                face_boxes.iter().any(|b| bb.x0 >= b.x0 && bb.y0 >= b.y0 && bb.x1 <= b.x1 && bb.y1 <= b.y1);
            if face_rules && !in_face_box {
                // Outside the face boxes an eye, brow or mouth part owns nothing:
                // the segmenter's patch there is the head's (T11 for the brows),
                // and a sliver of a region landing on the Mouth part rode the
                // head's underlay ABOVE the ink that should cover it. Its pixels
                // count for the largest other owner.
                if let Some(&(host, _)) = owners.iter().find(|(id, _)| !face_part[*id as usize]) {
                    let mut folded = 0usize;
                    owners.retain(|(id, n)| {
                        if face_part[*id as usize] {
                            folded += n;
                            false
                        } else {
                            true
                        }
                    });
                    if let Some(h) = owners.iter_mut().find(|(id, _)| *id == host) {
                        h.1 += folded;
                    }
                    owners.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                }
            }
            // Shares are taken over the pixels some part OWNS: a bag that is
            // 60% torso and 40% pixels no part owns is 100% torso, so it goes
            // whole to the torso instead of being clipped to a 60% sliver.
            let unowned = hist.get(&-1).copied().unwrap_or(0);
            let owned_total = total.saturating_sub(unowned);

            // **A region no part owns at all** (a bag, a strap, pocket pens, a
            // watch: drawn in the source, absent from every completed part)
            // goes whole to the part whose bone is nearest its centroid.
            //
            // T11: but it must also be NEAR: one whose centroid is farther than
            // `STRAY_DISTANCE_FACTOR` x the limb radius at that bone from the bone
            // segment and that is not attached to the figure (an unowned blob
            // touching an owned pixel, `stray_anchor_mask`) is a mark outside the
            // figure or a segmenter artefact, and is dropped.
            let Some(&(top_id, top_count)) = owners.first().filter(|_| owned_total > 0) else {
                let near = nearest_bone_part(&subpaths, completed, skel);
                let nearest = near.map(|(id, _)| id).or_else(|| {
                    *most_pixels_part.get_or_insert_with(|| {
                        let mut counts = vec![0usize; completed.parts.len()];
                        for &o in &owner {
                            if o >= 0 {
                                counts[o as usize] += 1;
                            }
                        }
                        counts.iter().enumerate().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(&a.0))).map(|(i, _)| i as i32)
                    })
                });
                if let (Some((id, distance)), Some(bone)) =
                    (near, near.and_then(|(id, _)| skel.bone(&completed.parts[id as usize].name)))
                {
                    let radius = limb_radius_at_head(bone, &dt, w, h, height) as f64;
                    let far = distance > STRAY_DISTANCE_FACTOR * radius;
                    let attached = far && {
                        let anchor =
                            stray_anchor.get_or_insert_with(|| stray_anchor_mask(&visible_rgba, &owner, w, h));
                        touches_anchor(&subpaths, anchor, w, h)
                    };
                    if stray_log && total >= 50 {
                        let bb = shape.path.bounding_box();
                        eprintln!(
                            "trace_character_whole: unowned {} ({total} px, bbox {:.0},{:.0}..{:.0},{:.0}) -> {:?}: {distance:.0} px from its bone, limb radius {radius:.0}, attached {attached}{}",
                            if is_ink { "ink" } else { "fill" },
                            bb.x0,
                            bb.y0,
                            bb.x1,
                            bb.y1,
                            completed.parts[id as usize].name,
                            if far && !attached { " -> DROPPED" } else { "" }
                        );
                    }
                    if far && !attached {
                        stray_dropped += 1;
                        stray_paths.push(shape.path.clone());
                        continue;
                    }
                }
                if let Some(id) = nearest {
                    if is_ink {
                        per_part_ink[id as usize].push(shape.clone());
                    } else {
                        per_part_shapes[id as usize].push(shape.clone());
                    }
                }
                continue;
            };
            let share = |n: usize| n as f64 / owned_total as f64;

            // **A region goes to its part whole unless it truly straddles.**
            // Cutting a region that sits 99% inside one part only adds a
            // stair-stepped seam, so a region with `WHOLE_REGION_SHARE` of its
            // area in one part, or a tiny one (under `WHOLE_REGION_SMALL_PX`),
            // is pushed unclipped to that part. A straddler is boolean-clipped
            // against the smoothed boundary of every part holding at least
            // `CLIP_MIN_SHARE` of its owned pixels (T9d: no pixel floor); if none does,
            // it goes whole to the top part (the clip is never widened to owners
            // holding a sliver).
            // I5: a face feature (its bounds inside a face box) goes whole to its
            // face-feature part when that holds a real share of it, else to the
            // top owner — never clipped (see `FACE_WHOLE_BOX_GROW`).
            let face_owner = if face_rules && in_face_box {
                let best_face = owners.iter().find(|(id, _)| face_part[*id as usize]);
                Some(match best_face {
                    Some(&(id, n)) if share(n) >= FACE_PART_MIN_SHARE => id,
                    _ => top_id,
                })
            } else {
                None
            };
            let pieces: Vec<(i32, BezPath)> = if let Some(id) = face_owner {
                vec![(id, shape.path.clone())]
            } else if share(top_count) >= WHOLE_REGION_SHARE || total < WHOLE_REGION_SMALL_PX {
                vec![(top_id, shape.path.clone())]
            } else {
                // Ascending share (`owners` is largest first, so reverse it): the
                // LARGEST owner is last and takes the remainder (T9b).
                // T9d: an owner qualifies with at least 1 % of the owned pixels
                // (no pixel floor; the "< 400 px² goes whole" rule above stays).
                let min_px = ((CLIP_MIN_SHARE * owned_total as f64).ceil() as usize).max(1);
                let qualifying: Vec<i32> =
                    owners.iter().rev().filter(|&&(_, n)| n >= min_px).map(|&(id, _)| id).collect();
                if qualifying.is_empty() {
                    vec![(top_id, shape.path.clone())]
                } else {
                    // **The pieces tile the region exactly.** Two parts'
                    // boundaries are simplified independently, so they do not
                    // coincide and wedges of the region would fall between
                    // clipped pieces and show the background. The owners go
                    // SMALLEST share first, the largest last (T9b: the sliver
                    // outside every simplified boundary must land on the
                    // dominant part, not the smallest): piece 1 = region ∩ B1;
                    // piece i = (region ∩ Bi) − (the pieces so far); the LAST
                    // (largest) owner takes everything still uncovered, so
                    // nothing of the region is lost.
                    let last = qualifying.len() - 1;
                    if is_ink && face_rules {
                        // I5 (colour path): ink pieces OVERLAP — each owner's piece is
                        // the shape within its grown boundary (`part_ink_clip`), one
                        // intersection each and nothing else: no subtraction, no
                        // union, no leftover (those booleans on the 4000-curve
                        // outline network left solid squares and bars behind). The
                        // grown boundaries cover the figure, so nothing is lost.
                        let mut out: Vec<(i32, BezPath)> = Vec::with_capacity(qualifying.len());
                        for &id in &qualifying {
                            let piece = buzz_geom::boolean(
                                &shape.path,
                                &part_ink_clip[id as usize],
                                BoolOp::Intersect,
                                bool_opts,
                            );
                            if !piece.is_empty() {
                                out.push((id, piece));
                            }
                        }
                        if out.is_empty() {
                            out.push((qualifying[last], shape.path.clone()));
                        }
                        out
                    } else {
                        let mut covered: Option<BezPath> = None;
                        let mut out: Vec<(i32, BezPath)> = Vec::with_capacity(qualifying.len());
                        for (k, &id) in qualifying.iter().enumerate() {
                            if k == last && face_rules {
                                break;
                            }
                            let base = if k == last {
                                shape.path.clone()
                            } else {
                                buzz_geom::boolean(
                                    &shape.path,
                                    &part_boundary[id as usize],
                                    BoolOp::Intersect,
                                    bool_opts,
                                )
                            };
                            let piece = match &covered {
                                Some(done) if !base.is_empty() => {
                                    buzz_geom::boolean(&base, done, BoolOp::Difference, bool_opts)
                                }
                                _ => base,
                            };
                            if piece.is_empty() {
                                continue;
                            }
                            covered = Some(match covered.take() {
                                Some(done) => buzz_geom::boolean(&done, &piece, BoolOp::Union, bool_opts),
                                None => piece.clone(),
                            });
                            out.push((id, piece));
                        }
                        if face_rules {
                            // I5: the REMAINDER (the region outside every exact piece:
                            // the rings the boundary simplification left, and the
                            // strips the segmenter gave the wrong part) goes sliver by
                            // sliver to the qualifying owner whose BONE is nearest its
                            // centroid. All of it to the largest owner left a ghost of
                            // Guddu's sleeve on the Hips, standing at the T-pose when
                            // the arm fell; by boundary distance a strip the Hips mask
                            // took along the sleeve top stayed with the Hips too.
                            let largest = qualifying[last];
                            let remainder = match &covered {
                                Some(done) => buzz_geom::boolean(&shape.path, done, BoolOp::Difference, bool_opts),
                                None => shape.path.clone(),
                            };
                            for (id, sliver) in split_remainder_by_bone(&remainder, &qualifying, largest, completed, skel) {
                                match out.iter_mut().find(|(pid, _)| *pid == id) {
                                    Some(piece) => piece.1 = buzz_geom::boolean(&piece.1, &sliver, BoolOp::Union, bool_opts),
                                    None => out.push((id, sliver)),
                                }
                            }
                        }
                        out
                    }
                }
            };
            if total >= 3000 && std::env::var_os("BUZZ_TRACE_INK_LOG").is_some() {
                let owners_s: Vec<String> = owners
                    .iter()
                    .map(|(id, n)| format!("{}:{:.0}%", completed.parts[*id as usize].name, 100.0 * share(*n)))
                    .collect();
                let pieces_s: Vec<String> = pieces
                    .iter()
                    .map(|(id, p)| {
                        let b = p.bounding_box();
                        format!(
                            "{}:{:.0}px² ({:.0},{:.0})-({:.0},{:.0})",
                            completed.parts[*id as usize].name,
                            p.area().abs(),
                            b.x0,
                            b.y0,
                            b.x1,
                            b.y1
                        )
                    })
                    .collect();
                eprintln!(
                    "trace_character_whole: {} region {total} px ({} unowned) {:?}: owners [{}] -> pieces [{}]",
                    if is_ink { "ink" } else { "fill" },
                    unowned,
                    shape.fill.as_ref().map(|f| f.color().to_rgba8().to_u8_array()),
                    owners_s.join(", "),
                    pieces_s.join(", ")
                );
            }
            for (id, piece_path) in pieces {
                // A3: the zero-area slivers the clip leaves are not artwork (see
                // `without_sliver_subpaths`); a piece that was nothing else is dropped.
                let piece_path = without_sliver_subpaths(&piece_path);
                if piece_path.elements().is_empty() {
                    continue;
                }
                let mut clipped_shape = shape.clone();
                clipped_shape.path = piece_path;

                // A filled shape near the line colour, covering a large area
                // of a part, should only exist where this part's own
                // completed pixels really are that dark — otherwise
                // `fill_enclosed` has swallowed the cloth *between* nearby
                // fold-line strokes into one solid dark patch (the "stray
                // dark filled band across pujariFront's kurta hem"
                // artefact: the dhoti's fold lines sit close enough together
                // that the space between them traced as one dark enclosed
                // region instead of cream cloth with thin dark accents on
                // top). Recolour it to the part's own dominant fill instead
                // of leaving a false-dark band on the artwork.
                // (Ink is dark by nature and keeps the colour the tracer gave it.)
                if let Some(fill) = clipped_shape.fill.as_mut().filter(|_| !is_ink) {
                    let area = hist.get(&id).copied().unwrap_or(0);
                    if area > DARK_FILL_AREA_GUARD_PX
                        && is_near_line_colour(fill.color(), line_color)
                        && !region_is_dark(
                            &completed.parts[id as usize].rgba,
                            w,
                            h,
                            &clipped_shape.path,
                        )
                    {
                        fill.paint = buzz_scene::object::Paint::Solid(part_dom_color[id as usize]);
                    }
                }

                if is_ink {
                    per_part_ink[id as usize].push(clipped_shape);
                } else {
                    per_part_shapes[id as usize].push(clipped_shape);
                }
            }
        } else if shape.stroke.is_some() {
            // A stroke is never cut at a part boundary: each subpath goes whole
            // to the part owning the majority of its points (only present with
            // `outline_strokes`).
            for sub in split_subpaths(&shape.path) {
                let mut votes: HashMap<i32, usize> = HashMap::new();
                for sp in flatten_subpaths(&sub) {
                    for p in &sp {
                        let id = owner_at(&owner, w, h, *p);
                        if id >= 0 {
                            *votes.entry(id).or_insert(0) += 1;
                        }
                    }
                }
                let Some((&id, _)) = votes.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) else { continue };
                let mut s = shape.clone();
                s.path = sub;
                per_part_shapes[id as usize].push(s);
            }
        }
    }

    // T11: what the stray rule dropped, and the per-part picture for review.
    if stray_log {
        eprintln!(
            "trace_character_whole: {stray_dropped} stray unowned shape(s) dropped (centroid over {STRAY_DISTANCE_FACTOR} limb radii from the nearest bone, touching no owned pixel)"
        );
    }

    // --- I5: the Mouth part keeps only its mouth FEATURES; everything else it
    // was handed (the segmenter's jaw patch: skin pieces, a sliver of the
    // collar's fill under the jaw line) goes to the head, under the head's
    // ink. (Bound separately as the mouth underlay it rode ABOVE that ink.)
    if face_rules {
        let head = completed.parts.iter().position(|p| p.name.to_lowercase().contains("head"));
        let mouth_box = crate::face::mouth_feature_box(&skel.face.mouth);
        if let Some(head) = head {
            for (id, part) in completed.parts.iter().enumerate() {
                if id == head || !part.name.to_lowercase().contains("mouth") {
                    continue;
                }
                let is_feature =
                    |s: &buzz_scene::ShapeData| mouth_box.is_some_and(|b| crate::face::is_mouth_feature(s, b));
                let (keep, moved): (Vec<_>, Vec<_>) =
                    std::mem::take(&mut per_part_shapes[id]).into_iter().partition(|s| is_feature(s));
                per_part_shapes[id] = keep;
                per_part_shapes[head].extend(moved);
                let (keep, moved): (Vec<_>, Vec<_>) =
                    std::mem::take(&mut per_part_ink[id]).into_iter().partition(|s| is_feature(s));
                per_part_ink[id] = keep;
                per_part_ink[head].extend(moved);
            }
        }
    }

    // --- T3: each part's hidden completion, traced behind its visible shapes.
    //
    // The pixels a part's completed picture holds that another part covers
    // (`alpha > 128` there, but someone else owns it) are what a limb shows
    // when it turns away from the part that hides it. They are traced as one
    // mask, filled with the part's dominant colour and put at the FRONT of the
    // part's shapes, behind every visible shape. This replaces the old
    // close-the-cut base fill of the whole simplified `part_boundary`, which
    // had the stair-step and the notch the boundary tolerance made;
    // `part_boundary` is now for clipping only.
    for (id, part) in completed.parts.iter().enumerate() {
        if skel.bone(&part.name).is_none() {
            continue;
        }
        // A face feature is never uncovered by a limb turning away: its
        // "hidden" pixels are just the segmenter's patch of lips or skin that
        // the head owns, and tracing them painted a dark lens over Raju's
        // smile (the Mouth part's dominant colour) in every rig so far.
        if is_eye_part(&part.name) || is_brow_part(&part.name) || part.name.to_lowercase().contains("mouth") {
            continue;
        }
        // I5: only within `reach` px (Chebyshev) of a pixel the part OWNS. The
        // segmenter's chest mask takes in the sleeves, so the chest's "hidden"
        // pixels were whole sleeves, and a cream sleeve-shaped patch stayed at
        // the T-pose on the chest when the arm swung down. What a part shows
        // when its neighbour turns away is the strip under the joint, not the
        // neighbour. (The T9e path keeps the whole hidden region.)
        let reach = if face_rules { (options.hidden_reach_fraction * height as f64).clamp(8.0, 40.0) as i64 } else { i64::MAX };
        let near: Option<Vec<bool>> = (reach != i64::MAX).then(|| {
            // Integral image of the owned mask; a pixel is near when the
            // (2·reach + 1)² box round it holds an owned pixel.
            let (wi, hi) = (w as usize, h as usize);
            let mut integral = vec![0u32; (wi + 1) * (hi + 1)];
            for y in 0..hi {
                let mut row = 0u32;
                for x in 0..wi {
                    row += (owner[y * wi + x] == id as i32) as u32;
                    integral[(y + 1) * (wi + 1) + x + 1] = integral[y * (wi + 1) + x + 1] + row;
                }
            }
            let r = reach as usize;
            (0..wi * hi)
                .map(|i| {
                    let (x, y) = (i % wi, i / wi);
                    let (x0, y0) = (x.saturating_sub(r), y.saturating_sub(r));
                    let (x1, y1) = ((x + r + 1).min(wi), (y + r + 1).min(hi));
                    let s = integral[y1 * (wi + 1) + x1] + integral[y0 * (wi + 1) + x0]
                        - integral[y0 * (wi + 1) + x1]
                        - integral[y1 * (wi + 1) + x0];
                    s > 0
                })
                .collect()
        });
        // B2: the `reach` gate is for ghost sleeves; a limb's extruded tube
        // (`CompletedPart::extruded`) runs to its joint whatever the distance.
        let n_px = (w * h) as usize;
        let mut hidden = vec![false; n_px];
        let mut any_hidden = false;
        for i in 0..n_px {
            let extruded = part.extruded.get(i).copied().unwrap_or(false);
            if part.rgba[i * 4 + 3] > 128 && owner[i] != id as i32 && (extruded || near.as_ref().is_none_or(|n| n[i])) {
                hidden[i] = true;
                any_hidden = true;
            }
        }
        if !any_hidden {
            continue;
        }
        if use_ink_colour {
            // B2: the hidden picture is the part's own completed colours on its
            // hidden pixels, plus a 2 px overlap into its own visible pixels (so
            // the seam has no crack), traced as Ink + colour with the character's
            // tuning: the fills go at the FRONT of the part's shapes (behind every
            // visible fill), the ink at the front of its ink.
            const OVERLAP_PX: usize = 2;
            let (wi, hi) = (w as usize, h as usize);
            let (mut bx0, mut by0, mut bx1, mut by1) = (wi, hi, 0usize, 0usize);
            for i in (0..n_px).filter(|&i| hidden[i]) {
                let (x, y) = (i % wi, i / wi);
                (bx0, by0, bx1, by1) = (bx0.min(x), by0.min(y), bx1.max(x), by1.max(y));
            }
            // Traced on a window round it (the tracer's transparent margin kept),
            // not the whole canvas: the trace is per part, and Ink + colour costs
            // in proportion to the picture's area.
            const MARGIN_PX: usize = 40;
            let (cx0, cy0) = (bx0.saturating_sub(OVERLAP_PX + MARGIN_PX), by0.saturating_sub(OVERLAP_PX + MARGIN_PX));
            let (cx1, cy1) = ((bx1 + OVERLAP_PX + MARGIN_PX + 1).min(wi), (by1 + OVERLAP_PX + MARGIN_PX + 1).min(hi));
            let (cw, ch) = (cx1 - cx0, cy1 - cy0);
            let mut picture = vec![0u8; cw * ch * 4];
            for y in by0.saturating_sub(OVERLAP_PX)..(by1 + OVERLAP_PX + 1).min(hi) {
                for x in bx0.saturating_sub(OVERLAP_PX)..(bx1 + OVERLAP_PX + 1).min(wi) {
                    let i = y * wi + x;
                    let take = hidden[i]
                        || (owner[i] == id as i32
                            && part.rgba[i * 4 + 3] > 128
                            && (y.saturating_sub(OVERLAP_PX)..(y + OVERLAP_PX + 1).min(hi)).any(|ny| {
                                (x.saturating_sub(OVERLAP_PX)..(x + OVERLAP_PX + 1).min(wi)).any(|nx| hidden[ny * wi + nx])
                            }));
                    if take {
                        let c = ((y - cy0) * cw + (x - cx0)) * 4;
                        picture[c..c + 4].copy_from_slice(&part.rgba[i * 4..i * 4 + 4]);
                    }
                }
            }
            let mut hidden_options = options.trace;
            hidden_options.mode = buzz_scene::trace::TraceMode::InkAndColour;
            hidden_options.drop_background = true;
            let mut hidden_report = buzz_scene::trace::trace(cw as u32, ch as u32, &picture, &hidden_options);
            let to_canvas = Affine::translate((cx0 as f64, cy0 as f64));
            for shape in &mut hidden_report.shapes {
                shape.path = to_canvas * shape.path.clone();
            }
            let n_colour = (hidden_report.fills + hidden_report.strokes).min(hidden_report.shapes.len());
            let mut hidden_fills = Vec::with_capacity(n_colour);
            let mut hidden_ink = Vec::new();
            for (k, shape) in hidden_report.shapes.into_iter().enumerate() {
                if k < n_colour {
                    hidden_fills.push(shape);
                } else {
                    hidden_ink.push(shape);
                }
            }
            per_part_shapes[id].splice(0..0, hidden_fills);
            per_part_ink[id].splice(0..0, hidden_ink);
            continue;
        }
        let mut hidden_mask = vec![0u8; n_px * 4];
        for i in (0..n_px).filter(|&i| hidden[i]) {
            hidden_mask[i * 4..i * 4 + 4].copy_from_slice(&[128, 128, 128, 255]);
        }
        let hidden_options = buzz_scene::trace::TraceOptions {
            mode: buzz_scene::trace::TraceMode::Outline,
            speckle: 16,
            ..Default::default()
        };
        let hidden_report = buzz_scene::trace::trace(w, h, &hidden_mask, &hidden_options);
        let hidden_shapes: Vec<buzz_scene::ShapeData> = hidden_report
            .shapes
            .into_iter()
            .filter(|s| s.fill.is_some())
            .map(|s| buzz_scene::ShapeData::filled(s.path, part_dom_color[id]))
            .collect();
        per_part_shapes[id].splice(0..0, hidden_shapes);
    }

    // --- Item 4: vector joint cap (disc + exposed-arc outline), per part ---
    //
    // The cap goes in `caps`, **not** in `shapes`: `assemble_character` binds
    // it to the part's bone as a rig part behind every other part, so it sits
    // behind both parts its joint joins and only shows where the joint opens.
    // See `TracedPart::caps`.
    let mut per_part_caps: Vec<Vec<buzz_scene::ShapeData>> = vec![Vec::new(); completed.parts.len()];
    for (id, part) in completed.parts.iter().enumerate() {
        if let Some(bone) = skel.bone(&part.name) {
            let mut radius = limb_radius_at_head(bone, &dt, w, h, height) as f64;
            let (jx, jy) = (bone.head.0 as f64, bone.head.1 as f64);

            // **No cap on a joint outside the character** (F7.7). A limb
            // foreshortened out of the picture (OldmanfrontWalk's far leg,
            // its foot hidden behind the knee) still gets an ankle from
            // DWPose, guessed below where the shin ends: a cap there was a
            // brown dot floating under the knee in every pose. The cap only
            // bridges a joint the drawing actually reaches, so a joint whose
            // pixel is outside the matte (distance 0) gets none.
            let (ix, iy) = (jx.round() as i64, jy.round() as i64);
            let inside_matte = ix >= 0
                && iy >= 0
                && (ix as u32) < w
                && (iy as u32) < h
                && dt.get((iy as u32 * w + ix as u32) as usize).is_some_and(|&d| d > 0.5);
            if !inside_matte {
                continue;
            }

            // Never let the cap balloon past this part's own local width at
            // the joint — measured directly from the pixels this part
            // actually owns in the composite, on the joint's own row rather
            // than the matte's local width, which (item 2) is capped at the
            // *bone's* own length but not at what this part itself draws:
            // a knee still wrapped in an undivided dhoti drape reads the
            // whole drape's width there, not the shin's. The oversized disc
            // that follows is invisible at rest (the neighbouring part's own
            // unrotated artwork covers the same pixels) but reads as a flat,
            // undetailed patch once this rigid part turns to a bent pose and
            // that neighbour has rotated away from underneath it — the
            // "white/pale ring at the knee/ankle" artefact.
            let jy_row = (jy.round() as i64).clamp(0, h as i64 - 1) as u32;
            let (mut min_x, mut max_x): (Option<i64>, Option<i64>) = (None, None);
            for px in 0..w {
                if owner[(jy_row * w + px) as usize] == id as i32 {
                    let x = px as i64;
                    min_x = Some(min_x.map_or(x, |m| m.min(x)));
                    max_x = Some(max_x.map_or(x, |m| m.max(x)));
                }
            }
            if let (Some(x0), Some(x1)) = (min_x, max_x) {
                let local_half_width = ((x1 - x0) as f64 * 0.5).max(2.0);
                radius = radius.min(local_half_width * 1.3);
            }
            // **The cap takes the colour of the part around its joint**, from
            // the pixels this part itself owns there, and the whole part's
            // dominant colour only when it owns too few to tell. The whole-part
            // colour is the wrong one for a part that is mostly something else
            // at the joint (the Hips are pale vest over most of their area but
            // brown trouser at the hip joint).
            let joint = Point::new(jx, jy);
            let cap_colour = match local_part_colour(part, &owner, id as i32, w, h, joint, radius) {
                Some(c) => nearest_palette_color(c, &palette),
                None => part_dom_color[id],
            };

            // **The cap never leaves the character's silhouette**: every ray
            // from the joint stops where the drawing stops, so the disc cannot
            // bulge out past the limb as a pale bump (nothing is drawn over it
            // to hide that, as it lies behind everything).
            let disc = cap_ring(&owner, w, h, joint, radius, 1.0, 0.0, std::f64::consts::TAU, 64);
            let mut disc_path = polyline_path(&disc);
            disc_path.close_path();
            per_part_caps[id].push(buzz_scene::ShapeData::filled(disc_path, cap_colour));

            // Exposed arc: the semicircle facing back toward the parent
            // joint (opposite the bone's own direction) — the side newly
            // added by the cap rather than already covered by this part's
            // own silhouette, so only it gets an outline stroke.
            let (dx, dy) = ((bone.tail.0 - bone.head.0) as f64, (bone.tail.1 - bone.head.1) as f64);
            let bone_angle = dy.atan2(dx);
            let exposed_center = bone_angle + std::f64::consts::PI;
            let arc_pts = cap_ring(
                &owner,
                w,
                h,
                joint,
                radius,
                1.0 + uw * 0.5,
                exposed_center - std::f64::consts::FRAC_PI_2,
                std::f64::consts::PI,
                24,
            );
            per_part_caps[id].push(buzz_scene::ShapeData::stroked(polyline_path(&arc_pts), line_color, uw));
        }
    }

    if let Some(path) = std::env::var_os("BUZZ_TRACE_PARTS_PNG") {
        write_parts_debug_png(&path, w, h, completed, &per_part_shapes, &per_part_ink, &per_part_caps, &stray_paths);
    }

    // Debug: `BUZZ_TRACE_SHAPES_LOG=x0,y0,x1,y1` lists every part's shapes
    // (fills, ink, caps) whose bounds meet that box: class, colour, area, bounds.
    if let Ok(spec) = std::env::var("BUZZ_TRACE_SHAPES_LOG") {
        let n: Vec<f64> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if n.len() == 4 {
            let want = Rect::new(n[0], n[1], n[2], n[3]);
            for (id, part) in completed.parts.iter().enumerate() {
                let classes = [("fill", &per_part_shapes[id]), ("ink", &per_part_ink[id]), ("cap", &per_part_caps[id])];
                for (class, list) in classes {
                    for (k, s) in list.iter().enumerate() {
                        let b = s.path.bounding_box();
                        if b.x1 < want.x0 || b.x0 > want.x1 || b.y1 < want.y0 || b.y0 > want.y1 {
                            continue;
                        }
                        eprintln!(
                            "shapes log: {:?} {class} {k}: {:?} area {:.0} bbox ({:.0},{:.0})-({:.0},{:.0})",
                            part.name,
                            s.fill.as_ref().map(|f| f.color().to_rgba8().to_u8_array()),
                            s.path.area().abs(),
                            b.x0,
                            b.y0,
                            b.x1,
                            b.y1
                        );
                    }
                }
            }
        }
    }

    let mut traced_parts = Vec::with_capacity(completed.parts.len());
    for (id, part) in completed.parts.iter().enumerate() {
        traced_parts.push(TracedPart {
            name: part.name.clone(),
            depth: part.depth,
            shapes: std::mem::take(&mut per_part_shapes[id]),
            ink: std::mem::take(&mut per_part_ink[id]),
            caps: std::mem::take(&mut per_part_caps[id]),
        });
    }

    Ok(TracedCharacter {
        parts: traced_parts,
        palette,
        uniform_width: uw,
    })
}

impl TracedPart {
    pub fn to_svg(&self, width: u32, height: u32) -> String {
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" height="{height}">"#
        );
        svg.push('\n');
        // The cap first: it sits behind the part's own artwork.
        for shape in self.caps.iter().chain(&self.shapes).chain(&self.ink) {
            let path_d = shape.path.to_svg();
            if let Some(fill) = &shape.fill {
                let c = fill.color().to_rgba8();
                svg.push_str(&format!(
                    r#"  <path d="{path_d}" fill="rgba({}, {}, {}, {:.2})" stroke="none" />"#,
                    c.r, c.g, c.b, c.a as f32 / 255.0
                ));
                svg.push('\n');
            }
            if let Some(stroke) = &shape.stroke {
                let c = stroke.color().to_rgba8();
                svg.push_str(&format!(
                    r#"  <path d="{path_d}" fill="none" stroke="rgba({}, {}, {}, {:.2})" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round" />"#,
                    c.r, c.g, c.b, c.a as f32 / 255.0, stroke.width
                ));
                svg.push('\n');
            }
        }
        svg.push_str("</svg>\n");
        svg
    }
}

impl TracedCharacter {
    pub fn to_svg(&self, width: u32, height: u32) -> String {
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" height="{height}">"#
        );
        svg.push('\n');
        let mut sorted_parts = self.parts.clone();
        sorted_parts.sort_by_key(|p| p.depth);

        for part in &sorted_parts {
            svg.push_str(&format!("  <!-- Part: {} (depth {}) -->\n", part.name, part.depth));
            for shape in part.shapes.iter().chain(&part.ink) {
                let path_d = shape.path.to_svg();
                if let Some(fill) = &shape.fill {
                    let c = fill.color().to_rgba8();
                    svg.push_str(&format!(
                        r#"  <path d="{path_d}" fill="rgba({}, {}, {}, {:.2})" stroke="none" />"#,
                        c.r, c.g, c.b, c.a as f32 / 255.0
                    ));
                    svg.push('\n');
                }
                if let Some(stroke) = &shape.stroke {
                    let c = stroke.color().to_rgba8();
                    svg.push_str(&format!(
                        r#"  <path d="{path_d}" fill="none" stroke="rgba({}, {}, {}, {:.2})" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round" />"#,
                        c.r, c.g, c.b, c.a as f32 / 255.0, stroke.width
                    ));
                    svg.push('\n');
                }
            }
        }
        svg.push_str("</svg>\n");
        svg
    }
}

/// Fully assembled and rigged character ready for library, stage, and Cast storage.
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledCharacter {
    pub name: String,
    pub character_symbol: Symbol,
    pub part_symbols: Vec<Symbol>,
    pub armature: ArmatureData,
    pub scene: Scene,
    pub face: Option<crate::face::FaceRigSymbols>,
}

impl AssembledCharacter {
    /// Save this character as an asset in the Cast folder of an AssetLibrary.
    pub fn save_to_cast(&self, assets: &mut buzz_doc::AssetLibrary) -> anyhow::Result<buzz_doc::Asset> {
        let unique_name = assets.unique_name(&self.name, "Cast");
        let saved = assets
            .save(&unique_name, "Cast", &self.scene)
            .map_err(|e| anyhow::anyhow!("Failed to save character to Cast: {e}"))?;
        Ok(saved)
    }

    /// Moves the pupils inside the composite eye symbols according to gaze offset (-1.0..=1.0).
    pub fn look_at(&mut self, gaze_x: f64, gaze_y: f64) {
        if let Some(face) = &mut self.face {
            crate::face::set_eye_look_at(&mut face.left_eye, gaze_x, gaze_y, face.max_pupil_travel);
            crate::face::set_eye_look_at(&mut face.right_eye, gaze_x, gaze_y, face.max_pupil_travel);
            self.scene.library_mut().insert(face.left_eye.clone());
            self.scene.library_mut().insert(face.right_eye.clone());
        }
    }

    /// Sets eyelid blink frame: 0.0 = fully open, 0.5 = half-closed, 1.0 = fully closed.
    pub fn set_blink(&mut self, t: f64) {
        if let Some(face) = &mut self.face {
            crate::face::set_eye_blink(&mut face.left_eye, t);
            crate::face::set_eye_blink(&mut face.right_eye, t);
            self.scene.library_mut().insert(face.left_eye.clone());
            self.scene.library_mut().insert(face.right_eye.clone());
        }
    }

    /// Applies an analyzed VisemeTrack onto a scene layer holding the mouth symbol instance.
    pub fn apply_lipsync(
        &mut self,
        track: &buzz_audio::VisemeTrack,
        sound_start: u32,
        layer: LayerId,
    ) -> buzz_act::lipsync::LipSyncReport {
        let mouth_id = self.face.as_ref().map(|f| f.mouth.id).unwrap_or(SymbolId(1));
        let placement = self
            .face
            .as_ref()
            .map(|f| Affine::translate((f.mouth_center.x, f.mouth_center.y)))
            .unwrap_or(Affine::IDENTITY);
        buzz_act::lipsync::write_track(&mut self.scene, track, sound_start, layer, mouth_id, placement)
    }
}

/// Helper mapping part names to `RigPattern::biped_full()` slots (0..17): the eleven
/// standard biped slots, then 11/12 Wrist L/R, 13/14 Ankle L/R, 15/16 Brow L/R.
///
/// The hand, foot and brow rules come BEFORE the forearm, shin and head rules, which
/// would otherwise take "Hand L" (it is a hand, not a forearm), "Foot L" and
/// "Left Eyebrow" (it holds "eye") into the elbow, knee and head slots. A hand, foot
/// or brow whose name carries no side falls through to the old rules as before.
fn map_part_to_biped_slot(name: &str) -> Option<usize> {
    let n = name.to_lowercase();
    let left = n.contains("left") || n.contains("_l") || n.ends_with(" l");
    let right = n.contains("right") || n.contains("_r") || n.ends_with(" r");
    if n.contains("hip") || n.contains("pelvis") || n.contains("waist") {
        Some(0)
    } else if n.contains("chest") || n.contains("torso") || n.contains("ribs") || n.contains("body") {
        Some(1)
    } else if (n.contains("hand") || n.contains("wrist") || n.contains("palm") || n.contains("finger")) && left {
        Some(11)
    } else if (n.contains("hand") || n.contains("wrist") || n.contains("palm") || n.contains("finger")) && right {
        Some(12)
    } else if (n.contains("foot") || n.contains("ankle") || n.contains("toe")) && left {
        Some(13)
    } else if (n.contains("foot") || n.contains("ankle") || n.contains("toe")) && right {
        Some(14)
    } else if n.contains("brow") && left {
        Some(15)
    } else if n.contains("brow") && right {
        Some(16)
    } else if n.contains("head") || n.contains("skull") || n.contains("face") || n.contains("eye") || n.contains("brow") || n.contains("mouth") {
        Some(2)
    } else if (n.contains("shoulder") || n.contains("arm") || n.contains("bicep")) && (n.contains("left") || n.contains("_l") || n.ends_with(" l")) {
        Some(3)
    } else if (n.contains("elbow") || n.contains("forearm") || n.contains("hand") || n.contains("wrist")) && (n.contains("left") || n.contains("_l") || n.ends_with(" l")) {
        Some(4)
    } else if (n.contains("shoulder") || n.contains("arm") || n.contains("bicep")) && (n.contains("right") || n.contains("_r") || n.ends_with(" r")) {
        Some(5)
    } else if (n.contains("elbow") || n.contains("forearm") || n.contains("hand") || n.contains("wrist")) && (n.contains("right") || n.contains("_r") || n.ends_with(" r")) {
        Some(6)
    } else if (n.contains("thigh") || n.contains("leg") || n.contains("upper leg")) && (n.contains("left") || n.contains("_l") || n.ends_with(" l")) {
        Some(7)
    } else if (n.contains("knee") || n.contains("shin") || n.contains("foot") || n.contains("ankle")) && (n.contains("left") || n.contains("_l") || n.ends_with(" l")) {
        Some(8)
    } else if (n.contains("thigh") || n.contains("leg") || n.contains("upper leg")) && (n.contains("right") || n.contains("_r") || n.ends_with(" r")) {
        Some(9)
    } else if (n.contains("knee") || n.contains("shin") || n.contains("foot") || n.contains("ankle")) && (n.contains("right") || n.contains("_r") || n.ends_with(" r")) {
        Some(10)
    } else {
        None
    }
}

/// One eyebrow as a bone, from its landmarks: the point nearest the face's centre line
/// (the inner end) to the point farthest from it (the outer end), by x. `centre_x` is
/// the face's centre line. `None` unless at least two confident landmarks sit at
/// different x.
fn brow_bone_from_landmarks(brow: &[(f32, f32, f32)], centre_x: f32) -> Option<(Point, Point)> {
    let confident: Vec<&(f32, f32, f32)> = brow.iter().filter(|p| p.2 > 0.2).collect();
    if confident.len() < 2 {
        return None;
    }
    let from_centre = |p: &&(f32, f32, f32)| (p.0 - centre_x).abs();
    let inner = confident.iter().copied().min_by(|a, b| from_centre(a).total_cmp(&from_centre(b)))?;
    let outer = confident.iter().copied().max_by(|a, b| from_centre(a).total_cmp(&from_centre(b)))?;
    if (outer.0 - inner.0).abs() < 1.0 {
        return None;
    }
    Some((
        Point::new(inner.0 as f64, inner.1 as f64),
        Point::new(outer.0 as f64, outer.1 as f64),
    ))
}

/// Per-slot `(head, tip)` for every `RigPattern::biped_full()` slot (17), read
/// directly off a detected `Skeleton` rather than fitted from each part's own outline.
///
/// Matches the side-by-side naming `map_part_to_biped_slot` uses, so a part
/// rides the bone on its own side of the body. `None` for a slot whose joint
/// was not confidently detected — [`buzz_act::assemble_from_joints`] falls
/// back to the pattern's own default for those, exactly as
/// [`buzz_act::assemble`] does for a slot nothing was dropped on.
///
/// Slots 0..11 are the standard biped. Since T10 the elbow and knee slots END at the
/// wrist and ankle, and the hand and foot are bones of their own: 11/12 run wrist ->
/// hand point, 13/14 ankle -> foot point, 15/16 are the brows, inner end -> outer end
/// by x (`None` without two confident landmarks; `assemble_character` then tries the
/// brow part's own bounds, and failing that the pattern's default applies).
pub(crate) fn biped_joints_from_skeleton(skel: &Skeleton) -> Vec<Option<(Point, Point)>> {
    let pt = |name: &str| -> Option<Point> {
        skel.joint(name)
            .filter(|j| j.confidence > 0.2)
            .map(|j| Point::new(j.x as f64, j.y as f64))
    };
    let pair = |head: &str, tip: &str| -> Option<(Point, Point)> { Some((pt(head)?, pt(tip)?)) };

    // The face's centre line: the nose, else the middle of the confident landmarks of
    // both brows together.
    let centre_x: Option<f32> = skel
        .joint("Nose")
        .filter(|j| j.confidence > 0.2)
        .map(|j| j.x)
        .or_else(|| {
            let xs: Vec<f32> = skel
                .face
                .left_brow
                .iter()
                .chain(&skel.face.right_brow)
                .filter(|p| p.2 > 0.2)
                .map(|p| p.0)
                .collect();
            (!xs.is_empty()).then(|| xs.iter().sum::<f32>() / xs.len() as f32)
        });
    let brow = |points: &[(f32, f32, f32)]| centre_x.and_then(|cx| brow_bone_from_landmarks(points, cx));

    vec![
        pair("Hips", "Chest"),         // 0 Hips
        pair("Chest", "Neck"),         // 1 Chest
        pair("Neck", "Head"),          // 2 Head — neck to the top of the head
        pair("Shoulder L", "Elbow L"), // 3 Shoulder L
        pair("Elbow L", "Wrist L"),    // 4 Elbow L — elbow to wrist
        pair("Shoulder R", "Elbow R"), // 5 Shoulder R
        pair("Elbow R", "Wrist R"),    // 6 Elbow R — elbow to wrist
        pair("Thigh L", "Knee L"),     // 7 Thigh L
        pair("Knee L", "Ankle L"),     // 8 Knee L — knee to ankle
        pair("Thigh R", "Knee R"),     // 9 Thigh R
        pair("Knee R", "Ankle R"),     // 10 Knee R — knee to ankle
        pair("Wrist L", "Hand L"),     // 11 Wrist L — the hand
        pair("Wrist R", "Hand R"),     // 12 Wrist R
        pair("Ankle L", "Foot L"),     // 13 Ankle L — the foot
        pair("Ankle R", "Foot R"),     // 14 Ankle R
        brow(&skel.face.left_brow),    // 15 Brow L — inner end to outer end
        brow(&skel.face.right_brow),   // 16 Brow R
    ]
}

/// A brow bone from the bounds of the brow part's own drawn shapes, for a brow with no
/// usable landmarks: across the box's vertical centre, the end nearer the face's centre
/// line first. `None` when the part draws nothing or no centre line is known.
fn brow_bone_from_bounds(bounds: Rect, skel: &Skeleton) -> Option<(Point, Point)> {
    let centre_x = skel
        .joint("Nose")
        .filter(|j| j.confidence > 0.2)
        .or_else(|| skel.joint("Head").filter(|j| j.confidence > 0.2))
        .map(|j| j.x as f64)?;
    if bounds.width() < 1.0 {
        return None;
    }
    let y = (bounds.y0 + bounds.y1) * 0.5;
    let (near, far) = if (bounds.x0 - centre_x).abs() <= (bounds.x1 - centre_x).abs() {
        (bounds.x0, bounds.x1)
    } else {
        (bounds.x1, bounds.x0)
    };
    Some((Point::new(near, y), Point::new(far, y)))
}

/// Helper finding the parent joint / pivot for a part from the skeleton.
fn part_pivot_point(part_name: &str, skel: &Skeleton, default_pt: Point) -> Point {
    let n = part_name.to_lowercase();
    if let Some(bone) = skel.bone(part_name) {
        return Point::new(bone.head.0 as f64, bone.head.1 as f64);
    }
    if n.contains("eye") || n.contains("brow") || n.contains("mouth") || n.contains("face") {
        if let Some(neck) = skel.joint("Neck") {
            return Point::new(neck.x as f64, neck.y as f64);
        }
        if let Some(head) = skel.joint("Head") {
            return Point::new(head.x as f64, head.y as f64);
        }
    }
    if n.contains("hand") && (n.contains("left") || n.contains("_l") || n.ends_with(" l")) {
        if let Some(b) = skel.bone("Elbow L") {
            return Point::new(b.tail.0 as f64, b.tail.1 as f64);
        }
    }
    if n.contains("hand") && (n.contains("right") || n.contains("_r") || n.ends_with(" r")) {
        if let Some(b) = skel.bone("Elbow R") {
            return Point::new(b.tail.0 as f64, b.tail.1 as f64);
        }
    }
    if n.contains("foot") && (n.contains("left") || n.contains("_l") || n.ends_with(" l")) {
        if let Some(b) = skel.bone("Knee L") {
            return Point::new(b.tail.0 as f64, b.tail.1 as f64);
        }
    }
    if n.contains("foot") && (n.contains("right") || n.contains("_r") || n.ends_with(" r")) {
        if let Some(b) = skel.bone("Knee R") {
            return Point::new(b.tail.0 as f64, b.tail.1 as f64);
        }
    }
    default_pt
}

/// Assembles traced character parts into Graphic symbols with registration points at parent joints,
/// creates a character symbol with parts arranged in depth order, fits an armature via autorig::assemble,
/// and returns the complete AssembledCharacter scene.
///
/// `limits` overrides the pattern's joint limits per slot (the user's saved
/// `rig_limits.json`); `None` keeps the built-in anatomical ones.
pub fn assemble_character(
    skel: &Skeleton,
    traced: &TracedCharacter,
    name: &str,
    stage_size: Option<(f64, f64)>,
    limits: Option<&buzz_rig::JointLimitDefaults>,
) -> anyhow::Result<AssembledCharacter> {
    let mut part_symbols = Vec::new();
    let mut part_objects = Vec::new();
    let mut pivots = Vec::new();

    // T10: the 17-slot biped — the eleven standard slots plus wrists, ankles and
    // brows, so hands, feet and eyebrows move on their own.
    let mut pattern = RigPattern::biped_full();
    if let Some(limits) = limits {
        pattern.apply_limits(limits);
    }

    // The DRAWN eyes are kept: the traced shapes that belong to each eye (see
    // `belongs_to_drawn_eye`) are moved out of their parts and into the
    // composite eye symbol built below (`crate::face::build_face_rig`), whose
    // white and pupil layers are those shapes and whose lid is cut from them.
    // Everything else, brows included, stays in its part untouched.
    let (left_eye_box, right_eye_box) = crate::face::eye_removal_boxes(skel, traced);
    let mut drawn_eyes = crate::face::DrawnEyes::default();
    let mut filtered_parts: Vec<TracedPart> = Vec::with_capacity(traced.parts.len());
    for part in &traced.parts {
        let mut kept = part.clone();
        kept.shapes.clear();
        kept.ink.clear();
        for shape in &part.shapes {
            if belongs_to_drawn_eye(shape, left_eye_box) {
                drawn_eyes.left.push(shape.clone());
            } else if belongs_to_drawn_eye(shape, right_eye_box) {
                drawn_eyes.right.push(shape.clone());
            } else {
                kept.shapes.push(shape.clone());
            }
        }
        // The ink inside an eye's box is that eye's outline (never its pupil).
        for shape in &part.ink {
            if belongs_to_drawn_eye(shape, left_eye_box) {
                drawn_eyes.left_ink.push(shape.clone());
            } else if belongs_to_drawn_eye(shape, right_eye_box) {
                drawn_eyes.right_ink.push(shape.clone());
            } else {
                kept.ink.push(shape.clone());
            }
        }
        filtered_parts.push(kept);
    }

    // 1. Build a Graphic symbol for each part
    for (i, part) in filtered_parts.iter().enumerate() {
        let sym_id = SymbolId(100 + i as u64);
        let sanitized = part.name.replace(' ', "_").to_lowercase();
        let sym_name = format!("{}_{}", name, sanitized);

        // Compute part bounds center as fallback pivot
        let mut part_bounds: Option<Rect> = None;
        for shape in part.shapes.iter().chain(&part.ink) {
            let b = shape.path.bounding_box();
            part_bounds = Some(match part_bounds {
                Some(all) => all.union(b),
                None => b,
            });
        }
        let default_pivot = part_bounds.map(|b| b.center()).unwrap_or(Point::ORIGIN);
        let pivot = part_pivot_point(&part.name, skel, default_pivot);
        pivots.push(pivot);

        let mut sym = Symbol::new(sym_id, sym_name, SymbolKind::Graphic);
        sym.registration = pivot;

        // Shift artwork relative to registration point
        let shift = Affine::translate((-pivot.x, -pivot.y));
        let shifted_shapes: Vec<buzz_scene::ShapeData> = part
            .shapes
            .iter()
            .map(|s| {
                let mut s_shift = s.clone();
                s_shift.path = shift * s_shift.path;
                s_shift
            })
            .collect();

        let layer_id = LayerId(i as u64 + 1);
        let mut inner_layer = Layer::new(layer_id, "Artwork", LayerKind::Normal);
        let inner_objects: Vec<Arc<Object>> = shifted_shapes
            .into_iter()
            .enumerate()
            .map(|(obj_idx, shape)| {
                let obj_id = ObjectId((sym_id.0 * 1000) + obj_idx as u64);
                Arc::new(Object::shape(obj_id, shape))
            })
            .collect();
        inner_layer.frames.set_objects(0, inner_objects);
        sym.layers.push_front(inner_layer);

        // T9: the ink, in an "Ink" layer ABOVE the artwork, in the same symbol (the
        // part stays one symbol, so rigging is unchanged).
        if !part.ink.is_empty() {
            let mut ink_layer = Layer::new(LayerId(10_000 + i as u64), "Ink", LayerKind::Normal);
            let ink_objects: Vec<Arc<Object>> = part
                .ink
                .iter()
                .enumerate()
                .map(|(k, s)| {
                    let mut s_shift = s.clone();
                    s_shift.path = shift * s_shift.path;
                    Arc::new(Object::shape(ObjectId(700_000_000 + sym_id.0 * 100_000 + k as u64), s_shift))
                })
                .collect();
            ink_layer.frames.set_objects(0, ink_objects);
            sym.layers.push_front(ink_layer);
        }

        // Object in document/stage space for autorig: the fills, then the ink on top.
        let drawn: Vec<&buzz_scene::ShapeData> = part.shapes.iter().chain(&part.ink).collect();
        let stage_obj = if drawn.len() == 1 {
            Object::shape(ObjectId(10000 + i as u64), drawn[0].clone())
        } else {
            let group_children: Vec<Arc<Object>> = drawn
                .iter()
                .enumerate()
                .map(|(obj_idx, shape)| {
                    let obj_id = ObjectId(20000 + (i as u64 * 100) + obj_idx as u64);
                    Arc::new(Object::shape(obj_id, (**shape).clone()))
                })
                .collect();
            Object::group(ObjectId(10000 + i as u64), group_children)
        };

        part_symbols.push(sym);
        part_objects.push(Arc::new(stage_obj));
    }

    // 2. Build the Character Symbol holding parts as layers in depth order
    let char_sym_id = SymbolId(1);
    let mut char_symbol = Symbol::new(char_sym_id, name.to_string(), SymbolKind::Graphic);
    let root_pivot = skel
        .joint("Hips")
        .map(|j| Point::new(j.x as f64, j.y as f64))
        .unwrap_or(Point::ORIGIN);
    char_symbol.registration = root_pivot;

    // Sort parts by depth ascending (lowest depth at bottom, highest on top)
    let mut depth_sorted_indices: Vec<usize> = (0..filtered_parts.len()).collect();
    depth_sorted_indices.sort_by_key(|&idx| filtered_parts[idx].depth);

    for &idx in &depth_sorted_indices {
        let part = &filtered_parts[idx];
        let layer_id = LayerId(idx as u64 + 1);
        let mut char_layer = Layer::new(layer_id, &part.name, LayerKind::Normal);
        let mut inst_obj = Object::instance_of(
            ObjectId(30000 + idx as u64),
            part_symbols[idx].id,
        );
        // Placed at pivot
        inst_obj.transform = Affine::translate((pivots[idx].x, pivots[idx].y));
        char_layer.frames.set_objects(0, vec![Arc::new(inst_obj)]);
        char_symbol.layers.push_front(char_layer);
    }

    // 3. Assemble the armature using autorig::assemble
    // Prepare (slot, artwork) pairs in back-to-front order
    let mut rig_parts_input: Vec<(usize, Arc<Object>)> = Vec::new();
    let mut remaining_subparts: Vec<(usize, Arc<Object>)> = Vec::new();

    // First, pass primary limbs for each slot so autorig fits bones accurately
    for &idx in &depth_sorted_indices {
        let part = &filtered_parts[idx];
        if let Some(slot) = map_part_to_biped_slot(&part.name) {
            // Check if this slot already has a primary artwork. The T10 slots (11..:
            // wrists, ankles, brows) are always bound AFTER the primaries, as the
            // hands, feet and brows always were (they used to share the elbow, knee
            // and head slots as subparts), so the paint order is exactly what it was
            // and a hand held in front of the face still draws over the head's hidden
            // completion. `assemble_from_joints` reads their bones from `joints`.
            if slot < 11 && !rig_parts_input.iter().any(|(s, _)| *s == slot) {
                rig_parts_input.push((slot, part_objects[idx].clone()));
            } else {
                remaining_subparts.push((slot, part_objects[idx].clone()));
            }
        }
    }

    // Assemble armature from primary slot parts, pivoting each bone at its
    // own detected joint rather than forcing it to start at its parent's tip
    // — see `biped_joints_from_skeleton`.
    let mut joints = biped_joints_from_skeleton(skel);
    // A brow with no usable landmarks takes its bone from the bounds of what its part
    // draws (a part that draws nothing leaves the slot to the pattern's default).
    for (slot, joint) in joints.iter_mut().enumerate().skip(15) {
        if joint.is_some() {
            continue;
        }
        let mut bounds: Option<Rect> = None;
        for part in filtered_parts.iter().filter(|p| map_part_to_biped_slot(&p.name) == Some(slot)) {
            for shape in part.shapes.iter().chain(&part.ink) {
                let b = shape.path.bounding_box();
                bounds = Some(bounds.map_or(b, |all| all.union(b)));
            }
        }
        *joint = bounds.and_then(|b| brow_bone_from_bounds(b, skel));
    }
    let mut armature = buzz_act::assemble_from_joints(&pattern, &joints, &rig_parts_input).ok_or_else(
        || anyhow::anyhow!("autorig::assemble_from_joints returned None for pattern {:?}", pattern.name),
    )?;

    // Bind any additional subparts (hands, feet, facial features) rigidly to their parent bone
    for (slot, subpart_obj) in remaining_subparts {
        armature.bind_rigid(subpart_obj, slot);
    }

    // **Joint caps go behind every part.** A cap bridges the gap that opens at a
    // joint when its part turns, so it belongs behind *both* parts the joint
    // joins, whatever their depth: bound to the part's own bone (it turns with
    // it) and moved to the very back of the rig's paint order. At rest it is
    // covered by the parts and invisible. Drawn at its own part's depth (as it
    // was, inside the part's shapes) it lay over any part below that one and
    // showed as a pale disc on the chest, hips and knees. See `TracedPart::caps`.
    let mut cap_count = 0;
    for &idx in &depth_sorted_indices {
        let part = &filtered_parts[idx];
        let Some(slot) = map_part_to_biped_slot(&part.name) else { continue };
        if part.caps.is_empty() {
            continue;
        }
        let cap_obj = if part.caps.len() == 1 {
            Object::shape(ObjectId(36000 + idx as u64), part.caps[0].clone())
        } else {
            let children: Vec<Arc<Object>> = part
                .caps
                .iter()
                .enumerate()
                .map(|(k, shape)| Arc::new(Object::shape(ObjectId(50000 + idx as u64 * 100 + k as u64), shape.clone())))
                .collect();
            Object::group(ObjectId(36000 + idx as u64), children)
        };
        armature.bind_rigid(Arc::new(cap_obj), slot);
        cap_count += 1;
    }
    armature.parts.rotate_right(cap_count);

    // 4. Construct Face Rig (Eyes, Pupils, Eyelids, Restyled Mouth)
    let face = crate::face::build_face_rig(
        skel,
        traced,
        name,
        &traced.palette,
        traced.uniform_width,
        500,
        &drawn_eyes,
    );

    // What the traced mouth part drew round the mouth (see
    // `FaceRigSymbols::mouth_underlay`) rides the head beneath the eyes and mouth.
    if !face.mouth_underlay.is_empty() {
        let children: Vec<Arc<Object>> = face
            .mouth_underlay
            .iter()
            .enumerate()
            .map(|(k, shape)| Arc::new(Object::shape(ObjectId(52000 + k as u64), shape.clone())))
            .collect();
        armature.bind_rigid(Arc::new(Object::group(ObjectId(35004), children)), 2);
    }

    // Bind eyes and mouth rigidly to Head bone (slot 2)
    let mut left_eye_obj = Object::instance_of(ObjectId(35001), face.left_eye.id);
    left_eye_obj.transform = Affine::translate((face.left_eye_center.x, face.left_eye_center.y));
    let mut right_eye_obj = Object::instance_of(ObjectId(35002), face.right_eye.id);
    right_eye_obj.transform = Affine::translate((face.right_eye_center.x, face.right_eye_center.y));
    let mut mouth_obj = Object::instance_of(ObjectId(35003), face.mouth.id);
    mouth_obj.transform = Affine::translate((face.mouth_center.x, face.mouth_center.y));
    // **The mouth holds one frame, Rest (frame 0).** An instance of the
    // ten-frame viseme symbol loops by default, so left alone the mouth cycled
    // through every viseme on any timeline (flickering Rest / lips-shut / ...)
    // until lip sync keyed it. Lip sync writes the frame it wants on each key.
    if let ObjectKind::Instance(inst) = &mut mouth_obj.kind {
        inst.loop_mode = buzz_scene::LoopMode::SingleFrame;
        inst.first_frame = 0;
    }

    armature.bind_rigid(Arc::new(left_eye_obj), 2);
    armature.bind_rigid(Arc::new(right_eye_obj), 2);
    armature.bind_rigid(Arc::new(mouth_obj), 2);

    // 5. Populate standard Pose Library on the armature
    armature.poses = crate::motion::PoseLibrary::standard().to_named_poses(&armature);

    // 6. Construct complete Scene
    let (stage_w, stage_h) = stage_size.unwrap_or((1024.0, 1024.0));
    let mut scene = Scene::default();
    scene.stage_mut().size = Size::new(stage_w, stage_h);

    // Add all symbols to library
    for sym in &part_symbols {
        scene.library_mut().insert(sym.clone());
    }
    scene.library_mut().insert(char_symbol.clone());

    // Add face symbols to library
    scene.library_mut().insert(face.left_eyelid.clone());
    scene.library_mut().insert(face.left_eye.clone());
    scene.library_mut().insert(face.right_eyelid.clone());
    scene.library_mut().insert(face.right_eye.clone());
    scene.library_mut().insert(face.mouth.clone());

    // Place the character on stage
    let char_layer = scene.add_layer(name, LayerKind::Normal);
    let armature_obj = Object {
        id: ObjectId(1),
        name: Some(name.to_string()),
        transform: Affine::IDENTITY,
        kind: ObjectKind::Armature(armature.clone()),
        locked: false,
        visible: true,
        filters: Vec::new(),
        blend: Blend::Normal,
        spatial: Default::default(),
        pivot: None,
        modifiers: Vec::new(),
        text: None,
        turnaround: Turnaround::default(),
    };
    scene.add_object(char_layer, armature_obj);

    // 7. Place every unbound "Prop N" part (a detached object in the source
    // photo — see `part_masks`' item 6) on its own layer, at its own
    // original position, entirely outside the armature: it was deliberately
    // never given a biped slot (`map_part_to_biped_slot` does not match
    // "Prop "), so it never entered `rig_parts_input`/`remaining_subparts`
    // above and is not rigged to any bone. The user can still see it, and
    // can rig or delete it themselves.
    for &idx in &depth_sorted_indices {
        let part = &filtered_parts[idx];
        if !part.name.starts_with("Prop ") {
            continue;
        }
        let prop_layer = scene.add_layer(&part.name, LayerKind::Normal);
        let mut prop_obj = Object::instance_of(ObjectId(40000 + idx as u64), part_symbols[idx].id);
        prop_obj.transform = Affine::translate((pivots[idx].x, pivots[idx].y));
        prop_obj.name = Some(part.name.clone());
        scene.add_object(prop_layer, prop_obj);
    }

    Ok(AssembledCharacter {
        name: name.to_string(),
        character_symbol: char_symbol,
        part_symbols,
        armature,
        scene,
        face: Some(face),
    })
}

#[cfg(test)]
mod trace_split_tests {
    use super::*;

    /// A3: a clipped piece keeps its outline and its hole, and loses the zero-area
    /// subpaths the boolean left (here two of the 32 in Raju's "Hand R" skin piece,
    /// verbatim: a 5.6 px line walked out and back, and three points 3e-6 px apart —
    /// the one whose seal stroke drew the 96 × 14 px band under the hand).
    #[test]
    fn a_clipped_piece_drops_the_slivers_the_boolean_left() {
        let mut path = Rect::new(43.0, 393.0, 191.0, 444.0).to_path(1e-9);
        let hole = Rect::new(150.0, 400.0, 160.0, 410.0).to_path(1e-9).reverse_subpaths();
        path.extend(hole);
        let slivers = BezPath::from_svg(
            "M78.22157218310082,432.99999992439376 L78.26726605745995,433.00000004360305 \
             L83.8107395395919,433.00000004360305 L83.81087245794976,432.99999992439376 Z \
             M92.41563286635125,432.1693449456416 L92.4156357273742,432.16934470722305 \
             L92.4156356081649,432.16934470722305 Z",
        )
        .unwrap();
        path.extend(slivers);
        assert_eq!(split_subpaths(&path).len(), 4);

        let kept = without_sliver_subpaths(&path);
        let subs = split_subpaths(&kept);
        assert_eq!(subs.len(), 2, "the outline and the hole stay, the slivers go: {}", kept.to_svg());
        assert!((kept.area().abs() - (148.0 * 51.0 - 100.0)).abs() < 1e-6, "area {}", kept.area());
        assert!(subs.iter().all(|s| s.bounding_box().width() >= 10.0));
    }

    #[test]
    fn cut_stroke_by_owner_splits_at_boundary() {
        let (w, h) = (10u32, 1u32);
        let mut owner = vec![0i32; 10];
        for o in owner.iter_mut().skip(5) {
            *o = 1;
        }
        let points: Vec<Point> = (0..10).map(|x| Point::new(x as f64, 0.0)).collect();
        let runs = cut_stroke_by_owner(&points, &owner, w, h);
        assert_eq!(runs.len(), 2, "must cut into exactly 2 runs at the owner boundary");
        assert_eq!(runs[0].0, 0);
        assert_eq!(runs[1].0, 1);
        // The boundary point is duplicated so the two cut runs still meet.
        assert_eq!(runs[0].1.last().copied(), runs[1].1.first().copied());
    }

    #[test]
    fn area_by_owner_splits_fill_area_between_two_parts() {
        let (w, h) = (10u32, 10u32);
        let mut owner = vec![-1i32; 100];
        for y in 0..10u32 {
            for x in 0..5u32 {
                owner[(y * 10 + x) as usize] = 0;
            }
            for x in 5..10u32 {
                owner[(y * 10 + x) as usize] = 1;
            }
        }
        let square = vec![vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ]];
        let (hist, total) = area_by_owner(&square, &owner, w, h);
        assert_eq!(total, 100);
        assert_eq!(*hist.get(&0).unwrap(), 50, "left half belongs to owner 0");
        assert_eq!(*hist.get(&1).unwrap(), 50, "right half belongs to owner 1");
    }
}

#[cfg(test)]
mod clean_regions_tests {
    use super::*;

    fn palette() -> Vec<peniko::Color> {
        // Darkest first, as `quantize_character_palette` sorts it.
        [[25, 20, 15], [200, 150, 110], [235, 220, 190], [90, 60, 40]]
            .iter()
            .map(|c| peniko::Color::from_rgb8(c[0], c[1], c[2]))
            .collect()
    }

    fn put(rgba: &mut [u8], w: u32, x: u32, y: u32, c: [u8; 3]) {
        let i = ((y * w + x) * 4) as usize;
        rgba[i..i + 3].copy_from_slice(&c);
        rgba[i + 3] = 255;
    }

    fn at(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    }

    /// A 500 x 400 piece of checked cloth: beige, thin orange threads every 24 px,
    /// scattered 2 x 2 dark specks, plus a long thin ink line and a big ink disc.
    fn cloth() -> (Vec<u8>, u32, u32, Vec<(u32, u32)>) {
        let (w, h) = (500u32, 400u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                put(&mut rgba, w, x, y, [235, 220, 190]);
            }
        }
        for y in 0..h {
            for x in 0..w {
                if x % 24 < 2 || y % 24 < 2 {
                    put(&mut rgba, w, x, y, [200, 150, 110]);
                }
            }
        }
        let mut specks = Vec::new();
        let mut seed = 12345u32;
        for _ in 0..200 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let x = 4 + (seed >> 8) % (w - 8);
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let y = 4 + (seed >> 8) % (h - 8);
            // Keep clear of the ink line and disc below.
            if (180..320).contains(&x) && (90..110).contains(&y) || (x as i32 - 400).pow(2) + (y as i32 - 250).pow(2) < 40 * 40 {
                continue;
            }
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                put(&mut rgba, w, x + dx, y + dy, [90, 60, 40]);
            }
            specks.push((x, y));
        }
        // A long thin ink line: a real outline stroke, 120 x 3 px.
        for x in 200..320 {
            for y in 100..103 {
                put(&mut rgba, w, x, y, [25, 20, 15]);
            }
        }
        // A big ink disc (a pupil, a moustache): radius 20.
        for y in 0..h {
            for x in 0..w {
                if (x as i32 - 400).pow(2) + (y as i32 - 250).pow(2) < 20 * 20 {
                    put(&mut rgba, w, x, y, [25, 20, 15]);
                }
            }
        }
        (rgba, w, h, specks)
    }

    /// **Q2.** Patterned cloth must not trace as speckle: tiny specks and thin
    /// coloured threads are merged into the cloth, while a long thin ink line and
    /// a big ink disc (real fine lines and shapes) are kept, and the picture
    /// comes out posterised to the palette.
    #[test]
    fn small_regions_merge_into_their_surroundings_and_real_lines_stay() {
        let (mut rgba, w, h, specks) = cloth();
        let palette = palette();
        let protect = vec![false; (w * h) as usize];
        let changed = clean_composite_regions(&mut rgba, w, h, &palette, palette[0], &protect);
        assert!(changed > 1000, "the specks and threads must be recoloured, changed {changed}");

        let beige = [235, 220, 190];
        for (x, y) in specks {
            assert_eq!(at(&rgba, w, x, y), beige, "the speck at ({x},{y}) must merge into the cloth");
        }
        // A thread (the orange line down x = 48..50, away from the specks' rows).
        assert_eq!(at(&rgba, w, 48, 300), beige, "a thin coloured thread must be smoothed away");

        // Real fine lines stay: the ink line and the ink disc.
        for x in [205, 260, 315] {
            assert_eq!(at(&rgba, w, x, 101), [25, 20, 15], "the long thin ink line must survive at x = {x}");
        }
        assert_eq!(at(&rgba, w, 400, 250), [25, 20, 15], "a big ink disc must survive");

        // Every opaque pixel is one of the palette colours.
        for i in 0..(w * h) as usize {
            let c = [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]];
            assert!(
                palette.iter().any(|p| {
                    let p = p.to_rgba8();
                    [p.r, p.g, p.b] == c
                }),
                "pixel {i} is {c:?}, not a palette colour"
            );
        }
    }

    /// The protected zone (the head and its features) keeps its fine marks: a dark
    /// speck of a few pixels, which cloth would merge, stays under `protect`
    /// (only dither the tracer drops anyway is merged there).
    #[test]
    fn protected_pixels_keep_small_marks_that_cloth_would_lose() {
        let (w, h) = (500u32, 400u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                put(&mut rgba, w, x, y, [235, 220, 190]);
            }
        }
        // A 5 x 5 darker mark (25 px): over the protected limit (8 px, the floor),
        // under the cloth limit (500 x 400 x 2.5e-4 = 50 px).
        for y in 200..205 {
            for x in 250..255 {
                put(&mut rgba, w, x, y, [200, 150, 110]);
            }
        }
        let palette = palette();
        let mut as_cloth = rgba.clone();
        clean_composite_regions(&mut as_cloth, w, h, &palette, palette[0], &vec![false; (w * h) as usize]);
        assert_eq!(at(&as_cloth, w, 252, 202), [235, 220, 190], "in cloth the mark merges");

        let mut protect = vec![false; (w * h) as usize];
        for y in 190..215 {
            for x in 240..265 {
                protect[(y * w + x) as usize] = true;
            }
        }
        let mut as_face = rgba.clone();
        clean_composite_regions(&mut as_face, w, h, &palette, palette[0], &protect);
        assert_eq!(at(&as_face, w, 252, 202), [200, 150, 110], "in the protected zone the mark stays");
    }

    /// Transparent pixels are never touched and never count as neighbours.
    #[test]
    fn transparent_pixels_are_left_alone() {
        let (mut rgba, w, h, _) = cloth();
        for y in 0..h {
            for x in 0..50 {
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[7, 7, 7, 0]);
            }
        }
        let palette = palette();
        let protect = vec![false; (w * h) as usize];
        clean_composite_regions(&mut rgba, w, h, &palette, palette[0], &protect);
        for y in [0, 100, 399] {
            let i = ((y * w + 10) * 4) as usize;
            assert_eq!(&rgba[i..i + 4], &[7, 7, 7, 0], "a transparent pixel is untouched");
        }
    }
}

#[cfg(test)]
mod under_paint_tests {
    use super::*;

    fn put(rgba: &mut [u8], w: usize, x: usize, y: usize, c: [u8; 3]) {
        let i = (y * w + x) * 4;
        rgba[i..i + 3].copy_from_slice(&c);
        rgba[i + 3] = 255;
    }

    fn at(rgba: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let i = (y * w + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    }

    #[test]
    fn dilate_square_grows_a_pixel_by_r_in_every_direction_and_clips_at_the_edge() {
        let (w, h) = (20usize, 12usize);
        let mut mask = vec![false; w * h];
        mask[5 * w + 10] = true;
        let grown = dilate_square(&mask, w, h, 4);
        for y in 0..h {
            for x in 0..w {
                let inside = (x as i64 - 10).abs() <= 4 && (y as i64 - 5).abs() <= 4;
                assert_eq!(grown[y * w + x], inside, "({x}, {y})");
            }
        }
        let corner = {
            let mut m = vec![false; w * h];
            m[0] = true;
            dilate_square(&m, w, h, 2)
        };
        assert_eq!(corner.iter().filter(|&&v| v).count(), 9, "a corner pixel grows into a 3 x 3 block");
        assert_eq!(dilate_square(&mask, w, h, 0), mask, "r = 0 changes nothing");
    }

    #[test]
    fn eye_and_head_parts_are_found_by_name_ignoring_case() {
        for name in ["Left Eye", "Right Eye", "RIGHT EYE", "left eye"] {
            assert!(is_eye_part(name), "{name}");
        }
        // T9e: the Eyebrow and Mouth parts are not protected as parts.
        for name in ["Left Eyebrow", "Right Eyebrow", "Mouth", "mouth", "Head", "Chest", "Upper Arm L", "Hand R"] {
            assert!(!is_eye_part(name), "{name}");
        }
        assert!(is_head_part("Head") && is_head_part("HEAD"));
        for name in ["Left Eye", "Mouth", "Chest", "Hand R"] {
            assert!(!is_head_part(name), "{name}");
        }
    }

    /// Skin A on the left, skin B on the right, a 3 px ink line at x 19..22 with a dark
    /// anti-aliased halo column each side (x 17, 18 and 22, 23). The paint set is the
    /// ink grown 1 px (x 18..23); the seeds are 3 px or more from the ink (x <= 16 and
    /// x >= 24), so the dark halo pixel at x 17 (2 px from the ink) is NEVER a seed:
    /// every painted pixel takes the clean skin colour of its nearer side, and the
    /// ring pixel at x 17 / 23 keeps its own (halo) colour.
    #[test]
    fn under_paint_takes_clean_skin_never_the_halo_and_the_nearer_side_wins() {
        let (w, h) = (41usize, 9usize);
        let (skin_a, skin_b, halo, ink_c) = ([200u8, 150, 110], [100u8, 150, 210], [120u8, 90, 70], [20u8, 20, 25]);
        let mut rgba = vec![0u8; w * h * 4];
        let mut ink = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let c = match x {
                    19..=21 => {
                        ink[y * w + x] = true;
                        ink_c
                    }
                    17 | 18 | 22 | 23 => halo,
                    0..=16 => skin_a,
                    _ => skin_b,
                };
                put(&mut rgba, w, x, y, c);
            }
        }
        let none = vec![false; w * h];
        let out = paint_under_ink(&rgba, &ink, &none, w as u32, h as u32);
        for y in 0..h {
            assert_eq!(at(&out, w, 18, y), skin_a, "the left halo pixel is repainted with skin A (row {y})");
            assert_eq!(at(&out, w, 19, y), skin_a, "the ink's left column is skin A");
            assert_eq!(at(&out, w, 21, y), skin_b, "the ink's right column is skin B");
            assert_eq!(at(&out, w, 22, y), skin_b, "the right halo pixel is repainted with skin B");
            assert_eq!(at(&out, w, 17, y), halo, "the ring pixel keeps its own colour");
            assert_eq!(at(&out, w, 23, y), halo, "the ring pixel keeps its own colour");
            assert_eq!(at(&out, w, 5, y), skin_a, "far pixels are untouched");
        }
        // A protected pixel neither donates nor receives.
        let mut protected = vec![false; w * h];
        for y in 0..h {
            protected[y * w + 20] = true;
        }
        let out = paint_under_ink(&rgba, &ink, &protected, w as u32, h as u32);
        for y in 0..h {
            assert_eq!(at(&out, w, 20, y), ink_c, "a protected pixel keeps its own colour");
        }
    }

    /// T9e: a 12 px brow bar (radius 6, UNDER `max_line_radius` 8 at this picture width)
    /// is a mass by compactness, so the paint set (the LINE mask grown 1 px) never reaches
    /// it: every brow pixel keeps its colour, while the 3 px pen line is repainted with skin.
    #[test]
    fn under_paint_from_the_line_mask_leaves_a_brow_mass_untouched() {
        let (w, h) = (1200usize, 120usize);
        let (skin, dark) = ([200u8, 150, 110], [30u8, 20, 20]);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let brow = (100..160).contains(&x) && (20..32).contains(&y);
                let pen = (100..400).contains(&x) && ((70..73).contains(&y) || (90..93).contains(&y));
                put(&mut rgba, w, x, y, if brow || pen { dark } else { skin });
            }
        }
        let options = character_ink_options(w as u32, h as u32, Some(peniko::Color::from_rgb8(30, 20, 20)));
        assert_eq!(options.max_line_radius, 8.0);
        let comps = buzz_scene::trace::ink_components(w as u32, h as u32, &rgba, &options);
        assert!(comps.mass_mask[26 * w + 130] && !comps.line_mask[26 * w + 130], "the brow is a mass");
        assert!(comps.line_mask[71 * w + 250], "the pen line is a line");
        let none = vec![false; w * h];
        let out = paint_under_ink(&rgba, &comps.line_mask, &none, w as u32, h as u32);
        for y in 20..32 {
            for x in 100..160 {
                assert_eq!(at(&out, w, x, y), dark, "brow pixel ({x}, {y}) is untouched");
            }
        }
        for y in 70..73 {
            assert_eq!(at(&out, w, 250, y), skin, "the pen line is under-painted with skin (row {y})");
        }
    }

    #[test]
    fn an_ink_pixel_no_seed_can_reach_keeps_its_own_colour() {
        // Everything within 2 px of the ink: no seed exists at all.
        let (w, h) = (5usize, 5usize);
        let mut rgba = vec![0u8; w * h * 4];
        let mut ink = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                put(&mut rgba, w, x, y, [30, 30, 30]);
            }
        }
        put(&mut rgba, w, 2, 2, [10, 10, 10]);
        ink[2 * w + 2] = true;
        let none = vec![false; w * h];
        assert_eq!(paint_under_ink(&rgba, &ink, &none, w as u32, h as u32), rgba);
    }
}
