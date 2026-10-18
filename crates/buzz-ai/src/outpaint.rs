//! **B1: outpainting a part that is cut by its neighbour**
//! (`docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md`, Stage B).
//!
//! A limb cut by the part over it (an upper arm under the chest, a shin under a
//! boot) is completed under that part by **extruding its cut-edge cross-section
//! along the bone** to the joint, with a round end: the two silhouette lines of
//! its outline continue straight, and so do its stripes and its shading. A part
//! that is not a limb takes the **nearest visible colour** of the part instead.
//!
//! **B1c** ([`measure_joint_cut`], [`apply_joint_cut`]) first makes every limb
//! joint a straight cut across the limb, so the part extruded starts from a clean
//! edge; **B1b** ([`borrow_counterpart`]) gives a limb that is almost wholly
//! hidden behind its L/R counterpart that counterpart's picture.
//!
//! Pure raster, deterministic (sequential scans in pixel order, no hash maps, no
//! parallel reductions), no model. Only pixels the caller allows (`allowed`:
//! hidden under a part that paints over this one, inside the matte) are painted.

/// The bone slots whose parts are limbs ([`outpaint_limb`]); every other bone part
/// (Chest, Hips, Head) takes [`nearest_visible_fill`]. The hand and foot ride the
/// Wrist and Ankle bones (`make_bone` in `character.rs`).
pub const LIMB_BONES: [&str; 12] = [
    "Shoulder L",
    "Elbow L",
    "Wrist L",
    "Shoulder R",
    "Elbow R",
    "Wrist R",
    "Thigh L",
    "Knee L",
    "Ankle L",
    "Thigh R",
    "Knee R",
    "Ankle R",
];

/// Whether `bone` (a bone name, any case) is one of [`LIMB_BONES`].
pub fn is_limb_bone(bone: &str) -> bool {
    LIMB_BONES.iter().any(|b| b.eq_ignore_ascii_case(bone.trim()))
}

/// The colour is sampled this far inside the cut (px), off its anti-aliased edge.
const CUT_INSET_PX: i64 = 2;
/// Stations in the median that smooths the visible profile.
const MEDIAN_STATIONS: i64 = 5;
/// How many visible stations inward of `t_cut + 2` the colour station may move.
const SLOPE_STATIONS: i64 = 8;
/// How far (px) a cross-section sample may look for a visible pixel when it
/// falls on a hidden one.
const SAMPLE_SEARCH_PX: i64 = 3;
/// Slack on the tube's edge tests (px), for pixel centres exactly on the edge.
const EDGE_EPS: f32 = 1e-3;
/// A cross-section is full when it is at least this share of the widest one
/// near the cut; the colour is taken from the first full one.
const FULL_SECTION: f32 = 0.9;
/// Alpha from which a pixel's straight colour is trusted as the limb's own.
const OPAQUE_ALPHA: u8 = 250;
/// How far past `t_cut` (px) a row's own first visible pixel may lie for the
/// tube to run on to it.
const ROW_REACH_PX: f32 = 12.0;

/// What [`outpaint_limb`] made of a limb part.
#[derive(Debug, Clone, PartialEq)]
pub struct OutpaintedLimb {
    /// The part's straight RGBA: its visible pixels as given, plus the tube's
    /// pixels painted opaque.
    pub rgba: Vec<u8>,
    /// The tube pixels painted (`w × h`); all `false` when the limb is not cut
    /// at either end.
    pub extruded: Vec<bool>,
}

/// The visible profile of a part in one bone frame: per 1 px station along the
/// axis, the visible extents across it, smoothed by a 5-station median.
struct Profile {
    /// Station index of `lo[0]` / `hi[0]` (`floor(t)` of the first visible pixel).
    k0: i64,
    /// Smoothed min / max `s` per station; `None` where a station has no visible pixel.
    lo: Vec<Option<f32>>,
    hi: Vec<Option<f32>>,
    /// The smallest `t` of any visible pixel.
    t_min: f32,
}

impl Profile {
    /// The profile of the visible pixels of `visible` (alpha > 128) in the frame
    /// with origin `o` and unit axis `u` (normal `(-u.y, u.x)`). `None` when no
    /// pixel is visible.
    fn measure(visible: &[u8], w: usize, h: usize, o: (f32, f32), u: (f32, f32)) -> Option<Self> {
        let nrm = (-u.1, u.0);
        let mut pts: Vec<(i64, f32)> = Vec::new();
        let mut t_min = f32::INFINITY;
        for y in 0..h {
            for x in 0..w {
                if visible[(y * w + x) * 4 + 3] <= 128 {
                    continue;
                }
                let (dx, dy) = (x as f32 - o.0, y as f32 - o.1);
                let t = dx * u.0 + dy * u.1;
                let s = dx * nrm.0 + dy * nrm.1;
                t_min = t_min.min(t);
                pts.push((t.floor() as i64, s));
            }
        }
        if pts.is_empty() {
            return None;
        }
        let k0 = pts.iter().map(|p| p.0).min().unwrap_or(0);
        let k1 = pts.iter().map(|p| p.0).max().unwrap_or(0);
        let n = (k1 - k0 + 1) as usize;
        let mut raw_lo: Vec<Option<f32>> = vec![None; n];
        let mut raw_hi: Vec<Option<f32>> = vec![None; n];
        for &(k, s) in &pts {
            let i = (k - k0) as usize;
            raw_lo[i] = Some(raw_lo[i].map_or(s, |v| v.min(s)));
            raw_hi[i] = Some(raw_hi[i].map_or(s, |v| v.max(s)));
        }
        let half = MEDIAN_STATIONS / 2;
        let smooth = |raw: &[Option<f32>]| -> Vec<Option<f32>> {
            (0..n as i64)
                .map(|i| {
                    raw[i as usize]?;
                    let mut window: Vec<f32> = ((i - half).max(0)..=(i + half).min(n as i64 - 1))
                        .filter_map(|j| raw[j as usize])
                        .collect();
                    window.sort_by(f32::total_cmp);
                    Some(window[(window.len() - 1) / 2])
                })
                .collect()
        };
        Some(Self { k0, lo: smooth(&raw_lo), hi: smooth(&raw_hi), t_min })
    }

    /// The smoothed `(lo, hi)` at station `k`, if it has visible pixels.
    fn at(&self, k: i64) -> Option<(f32, f32)> {
        let i = k - self.k0;
        if i < 0 || i as usize >= self.lo.len() {
            return None;
        }
        Some((self.lo[i as usize]?, self.hi[i as usize]?))
    }
}

/// The lower median of `v` (sorted in place); 0 when empty.
fn lower_median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[(v.len() - 1) / 2]
}

/// **B1: completes a limb part cut by its neighbour** by extruding its cut-edge
/// cross-section along the bone to the joint, with a round end.
///
/// `visible` is the part's own straight RGBA (`part.rgba` before completion,
/// `source`'s size); `allowed` the pixels it may paint (hidden under a part that
/// paints over it and inside the matte, `complete_parts`' hole rule); `head` /
/// `tail` the bone (parent joint first), in pixels; `source` the picture the
/// colours are taken from.
///
/// 1. Bone frame: `t` along the axis from the joint, `s` across it.
/// 2. Visible profile: per 1 px station the visible extents `lo(t)..hi(t)`,
///    5-station median. The cut is at the head when the visible pixels nearest
///    the head stop at `t_cut > 0`; otherwise, if they stop short of the tail,
///    at the tail (the same with head and tail swapped); otherwise there is no cut.
/// 3. Tube (B1c): from `t_cut` to the joint, centred on the profile's centre at
///    the cut; past the joint a round end of radius
///    `r_end = min(hw0, distance from the joint to the nearest non-matte pixel − 1)`
///    (`hw0` the half-width at the cut, the joint taken on the tube's centre
///    line), so the silhouette never clips the end to a straight edge; the
///    half-width tapers linearly from `hw0` at the cut to `r_end` at the joint.
/// 4. Colour: a tube pixel `(t, s)` takes the visible colour at the station
///    `t_cut + 2` (inward, off the anti-aliased cut), at `s` rescaled by the
///    width ratio of the two stations: the outline ink, the stripes and the
///    shading continue; no ink is drawn across the round end.
///
/// Only `tube ∧ allowed` is painted (never a visible pixel). `matte` (`w × h`,
/// inside the character) sizes the round end. `None` when the part has no
/// visible pixel to measure a cross-section from.
pub fn outpaint_limb(
    visible: &[u8],
    allowed: &[bool],
    matte: &[bool],
    head: (f32, f32),
    tail: (f32, f32),
    source: &image::RgbaImage,
) -> Option<OutpaintedLimb> {
    outpaint_limb_end(visible, allowed, matte, head, tail, source, LimbEnd::Either)
}

/// Which end(s) of a limb [`outpaint_limb_end`] extrudes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LimbEnd {
    /// The head when it is cut, else the tail (B1).
    Either,
    /// The head only, when it is cut.
    Head,
    /// The tail, from at least this far (px) from the tail joint (B4b-3).
    Tail(f32),
}

/// **B4b-3: a limb under the part below it at a round joint** (the thigh under
/// the shin's round end, [`round_over_end`]) is extruded at both ends: at its
/// head as [`outpaint_limb`] does when that end is cut, and at its tail from the
/// cross-section `tail_cut` px up from the tail joint (the top of the over part's
/// round end, where its full width starts) down to the joint, with a round end
/// past it, on the pixels it may paint (so the tube is clipped to the matte).
pub fn outpaint_limb_both_ends(
    visible: &[u8],
    allowed: &[bool],
    matte: &[bool],
    head: (f32, f32),
    tail: (f32, f32),
    source: &image::RgbaImage,
    tail_cut: f32,
) -> Option<OutpaintedLimb> {
    let first = outpaint_limb_end(visible, allowed, matte, head, tail, source, LimbEnd::Head)?;
    let Some(second) = outpaint_limb_end(&first.rgba, allowed, matte, head, tail, source, LimbEnd::Tail(tail_cut)) else {
        return Some(first);
    };
    let extruded = first.extruded.iter().zip(&second.extruded).map(|(a, b)| *a || *b).collect();
    Some(OutpaintedLimb { rgba: second.rgba, extruded })
}

fn outpaint_limb_end(
    visible: &[u8],
    allowed: &[bool],
    matte: &[bool],
    head: (f32, f32),
    tail: (f32, f32),
    source: &image::RgbaImage,
    end: LimbEnd,
) -> Option<OutpaintedLimb> {
    let (w, h) = (source.width() as usize, source.height() as usize);
    let n = w * h;
    if visible.len() != n * 4 || allowed.len() != n || matte.len() != n || !(0..n).any(|i| visible[i * 4 + 3] > 128) {
        return None;
    }
    let mut out = OutpaintedLimb { rgba: visible.to_vec(), extruded: vec![false; n] };
    let (ax, ay) = (tail.0 - head.0, tail.1 - head.1);
    let len = (ax * ax + ay * ay).sqrt();
    if len < 1e-3 {
        return Some(out);
    }
    let u_head = (ax / len, ay / len);
    // The head side first; else the tail side, roles swapped.
    let frames = [(head, u_head), (tail, (-u_head.0, -u_head.1))];
    let (frames, min_cut): (&[((f32, f32), (f32, f32))], f32) = match end {
        LimbEnd::Either => (&frames[..], 0.0),
        LimbEnd::Head => (&frames[..1], 0.0),
        LimbEnd::Tail(c) => (&frames[1..], c.max(0.0)),
    };
    let Some((origin, u, profile)) = frames.iter().copied().find_map(|(o, u)| {
        let p = Profile::measure(visible, w, h, o, u)?;
        (p.t_min > 0.0 || min_cut > 0.0).then_some((o, u, p))
    }) else {
        return Some(out);
    };
    let nrm = (-u.1, u.0);

    // The cut station and its cross-section (B4b-3: no nearer the joint than
    // `min_cut`, at the first station from there that has visible pixels).
    let Some(k_cut) = (profile.k0.max(min_cut.ceil() as i64)..profile.k0.max(min_cut.ceil() as i64) + 2 * SLOPE_STATIONS)
        .find(|&k| profile.at(k).is_some())
    else {
        return Some(out);
    };
    let t_cut = k_cut as f32;
    let (lo0, hi0) = profile.at(k_cut)?;
    let c0 = 0.5 * (lo0 + hi0);
    let hw0 = 0.5 * (hi0 - lo0);
    // B1c: the round end fits inside the matte, and the tube tapers linearly to it.
    let end_centre = (origin.0 + c0 * nrm.0, origin.1 + c0 * nrm.1);
    let r_end = (distance_to_outside(matte, w, h, end_centre, hw0 + 2.0) - 1.0).clamp(0.0, hw0);
    let half_width_at = |t: f32| -> f32 {
        if t >= t_cut || t_cut <= 0.0 {
            hw0
        } else {
            r_end + (hw0 - r_end) * (t / t_cut).max(0.0)
        }
    };

    // The source station: `t_cut + 2`, else the nearest visible station inward of
    // it, else outward.
    // A station only counts when its cross-section is full: a stroke's corner
    // that pokes out past a hand-made cut (Extend Part on a lasso-cut symbol)
    // starts the profile with a few narrow stations, whose colours are the
    // outline's and the anti-aliased edge's, not the limb's.
    let width_at = |k: i64| profile.at(k).map(|(lo, hi)| hi - lo);
    let full = (k_cut..k_cut + CUT_INSET_PX + 2 * SLOPE_STATIONS).filter_map(width_at).fold(0.0f32, f32::max);
    let is_full = |k: i64| width_at(k).is_some_and(|wd| wd >= FULL_SECTION * full);
    let k_src = (k_cut + CUT_INSET_PX..k_cut + CUT_INSET_PX + 2 * SLOPE_STATIONS)
        .find(|&k| is_full(k))
        .or_else(|| (k_cut + CUT_INSET_PX..k_cut + CUT_INSET_PX + SLOPE_STATIONS).find(|&k| profile.at(k).is_some()))
        .or_else(|| (k_cut..k_cut + CUT_INSET_PX).rev().find(|&k| profile.at(k).is_some()))?;
    let (lo_s, hi_s) = profile.at(k_src)?;
    let (c_s, hw_s) = (0.5 * (lo_s + hi_s), 0.5 * (hi_s - lo_s));

    // The cross-section's colours at the source station, 1 px apart across it.
    let samples = ((hi_s - lo_s).ceil().max(0.0) as usize) + 1;
    let step = if samples > 1 { (hi_s - lo_s) / (samples - 1) as f32 } else { 0.0 };
    let visible_at = |x: i64, y: i64| -> bool {
        x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && visible[(y as usize * w + x as usize) * 4 + 3] > 128
    };
    let mut colours: Vec<Option<[u8; 3]>> = (0..samples)
        .map(|j| {
            let s = if samples > 1 { lo_s + j as f32 * step } else { c_s };
            let px = origin.0 + k_src as f32 * u.0 + s * nrm.0;
            let py = origin.1 + k_src as f32 * u.1 + s * nrm.1;
            let (cx, cy) = (px.round() as i64, py.round() as i64);
            // The nearest visible pixel to the sample point (scan order on a
            // tie), a fully opaque one when there is one: an anti-aliased edge
            // pixel's straight colour is not the limb's.
            let mut best: Option<(bool, f32, i64, i64)> = None;
            for dy in -SAMPLE_SEARCH_PX..=SAMPLE_SEARCH_PX {
                for dx in -SAMPLE_SEARCH_PX..=SAMPLE_SEARCH_PX {
                    let (x, y) = (cx + dx, cy + dy);
                    if !visible_at(x, y) {
                        continue;
                    }
                    let opaque = visible[(y as usize * w + x as usize) * 4 + 3] >= OPAQUE_ALPHA;
                    let d = (x as f32 - px).powi(2) + (y as f32 - py).powi(2);
                    if best.is_none_or(|(bo, bd, _, _)| (opaque && !bo) || (opaque == bo && d < bd)) {
                        best = Some((opaque, d, x, y));
                    }
                }
            }
            let best = best.map(|(_, d, x, y)| (d, x, y));
            best.map(|(_, x, y)| {
                let p = source.get_pixel(x as u32, y as u32).0;
                [p[0], p[1], p[2]]
            })
        })
        .collect();
    let known: Vec<usize> = (0..samples).filter(|&j| colours[j].is_some()).collect();
    if known.is_empty() {
        return Some(out);
    }
    for j in 0..samples {
        if colours[j].is_none() {
            let nearest = known.iter().copied().min_by_key(|&k| (k.abs_diff(j), k)).unwrap_or(known[0]);
            colours[j] = colours[nearest];
        }
    }

    // Per row across the tube (`s` rounded), the first visible `t`: a cut that
    // is not square to the bone (a lasso cut) starts later on some rows than
    // `t_cut`, and the tube runs on to it there, so no gap is left.
    let s_lo = (c0 - hw0 - 2.0).floor() as i64;
    let mut row_start = vec![f32::INFINITY; ((c0 + hw0 + 2.0).ceil() as i64 - s_lo + 1).max(1) as usize];
    for y in 0..h {
        for x in 0..w {
            if visible[(y * w + x) * 4 + 3] <= 128 {
                continue;
            }
            let (dx, dy) = (x as f32 - origin.0, y as f32 - origin.1);
            let t = dx * u.0 + dy * u.1;
            let j = (dx * nrm.0 + dy * nrm.1).round() as i64 - s_lo;
            if (0..row_start.len() as i64).contains(&j) && t < t_cut + ROW_REACH_PX {
                row_start[j as usize] = row_start[j as usize].min(t);
            }
        }
    }
    let tube_end = |s: f32| -> f32 {
        let j = s.round() as i64 - s_lo;
        let row = if (0..row_start.len() as i64).contains(&j) { row_start[j as usize] } else { f32::INFINITY };
        if row.is_finite() { (row + 0.5).max(t_cut + 1.0) } else { t_cut + 1.0 }
    };

    // The tube's bounds on the picture.
    let reach_s = hw0 + 1.0;
    let t_far = t_cut + ROW_REACH_PX + 1.0;
    let corners = [(-r_end - 1.0, c0 - reach_s), (-r_end - 1.0, c0 + reach_s), (t_far, c0 - reach_s), (t_far, c0 + reach_s)];
    let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for (t, s) in corners {
        let (x, y) = (origin.0 + t * u.0 + s * nrm.0, origin.1 + t * u.1 + s * nrm.1);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let xr = (x0.floor().max(0.0) as usize, (x1.ceil().max(0.0) as usize).min(w.saturating_sub(1)));
    let yr = (y0.floor().max(0.0) as usize, (y1.ceil().max(0.0) as usize).min(h.saturating_sub(1)));

    for y in yr.0..=yr.1 {
        for x in xr.0..=xr.1 {
            let i = y * w + x;
            if !allowed[i] || visible[i * 4 + 3] > 128 {
                continue;
            }
            let (dx, dy) = (x as f32 - origin.0, y as f32 - origin.1);
            let t = dx * u.0 + dy * u.1;
            let s = dx * nrm.0 + dy * nrm.1;
            let hw = if t >= 0.0 {
                // The tube's body, from the joint up to the cut station.
                let hw = half_width_at(t);
                if t >= tube_end(s) || (s - c0).abs() > hw + EDGE_EPS {
                    continue;
                }
                hw
            } else {
                // The round end past the joint.
                if t * t + (s - c0) * (s - c0) > r_end * r_end + EDGE_EPS {
                    continue;
                }
                r_end
            };
            let s_src = if hw > EDGE_EPS { c_s + (s - c0) * (hw_s / hw) } else { c_s };
            let j = if step > 0.0 { ((s_src - lo_s) / step).round().clamp(0.0, (samples - 1) as f32) as usize } else { 0 };
            let Some(rgb) = colours[j] else { continue };
            out.rgba[i * 4..i * 4 + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            out.extruded[i] = true;
        }
    }
    Some(out)
}

/// The distance from `p` to the centre of the nearest pixel outside `matte`
/// (`w × h`; pixels off the picture count as outside), looked for within
/// `max_r`; `max_r` when there is none that close.
fn distance_to_outside(matte: &[bool], w: usize, h: usize, p: (f32, f32), max_r: f32) -> f32 {
    let r = max_r.ceil().max(0.0) as i64 + 1;
    let (cx, cy) = (p.0.round() as i64, p.1.round() as i64);
    let mut best = max_r;
    for y in cy - r..=cy + r {
        for x in cx - r..=cx + r {
            let outside = x < 0 || y < 0 || x as usize >= w || y as usize >= h || !matte[y as usize * w + x as usize];
            if outside {
                best = best.min(((x as f32 - p.0).powi(2) + (y as f32 - p.1).powi(2)).sqrt());
            }
        }
    }
    best
}

/// B1c: the cut's width test compares a station with the median of this many
/// stations after it.
const CUT_LOOKAHEAD_STATIONS: i64 = 10;
/// B1c: the cut is the first station whose width reaches this share of that median.
const CUT_WIDTH_SHARE: f32 = 0.85;
/// B1c: the cut lies within this range (fractions of the bone's length) of the joint.
const CUT_CLAMP: (f32, f32) = (0.1, 0.4);
/// B1c: the band a cut reassigns pixels in is the matte's cross-section, at most
/// this multiple of the limb's own half-width either side of its centre line.
const BAND_CLIP: f32 = 1.25;

/// The distance from `p` to the segment `a`–`b`.
fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let k = if l2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ((p.0 - a.0 - k * dx).powi(2) + (p.1 - a.1 - k * dy).powi(2)).sqrt()
}

/// **B1c: a clean, straight cut at a joint**, perpendicular to the joint's
/// *child* bone (the thigh at the hip, the shin at the knee, the upper arm at the
/// shoulder, the forearm at the elbow, the hand / foot at the wrist / ankle).
/// Measured by [`measure_joint_cut`], applied by [`apply_joint_cut`].
#[derive(Debug, Clone)]
pub struct JointCut {
    /// The joint (the child bone's head).
    origin: (f32, f32),
    /// The child bone's unit axis.
    u: (f32, f32),
    /// The child bone's length.
    len: f32,
    /// The parent part's bone (head, tail).
    parent_bone: ((f32, f32), (f32, f32)),
    /// The cut's distance from the joint along the child bone (px, a whole station).
    pub t_cut: f32,
    /// Station of `band[0]`.
    k_lo: i64,
    /// Per station, the band's `(lo, hi)` across the bone (pixel centres), `None`
    /// where the limb's centre line is outside the matte.
    band: Vec<Option<(f32, f32)>>,
}

impl JointCut {
    /// `(t, s)` of the pixel centre `(x, y)`: along the child bone from the
    /// joint, and across it.
    fn frame(&self, x: usize, y: usize) -> (f32, f32) {
        let (dx, dy) = (x as f32 - self.origin.0, y as f32 - self.origin.1);
        (dx * self.u.0 + dy * self.u.1, -dx * self.u.1 + dy * self.u.0)
    }

    /// Whether `(t, s)` lies within the band (half a pixel of slack).
    fn in_band(&self, t: f32, s: f32) -> bool {
        let i = t.floor() as i64 - self.k_lo;
        if i < 0 || i as usize >= self.band.len() {
            return false;
        }
        self.band[i as usize].is_some_and(|(lo, hi)| s >= lo - 0.5 && s <= hi + 0.5)
    }

    /// `|s|` (the distance from the child bone's axis) of the pixel `(x, y)` when
    /// it lies on the cut's far side (`t_cut ≤ t ≤ len`) within the band.
    pub fn far_side_distance(&self, x: usize, y: usize) -> Option<f32> {
        let (t, s) = self.frame(x, y);
        (t >= self.t_cut && t <= self.len && self.in_band(t, s)).then_some(s.abs())
    }

    /// The pixel bounds `(x0, y0, x1, y1)` (inclusive, clamped to `w × h`) of the
    /// band for `t` in `t0..=t1`.
    fn bounds(&self, t0: f32, t1: f32, w: usize, h: usize) -> (usize, usize, usize, usize) {
        let (s0, s1) = self
            .band
            .iter()
            .flatten()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &(lo, hi)| (a.min(lo - 1.0), b.max(hi + 1.0)));
        if !s0.is_finite() || !s1.is_finite() {
            return (1, 1, 0, 0);
        }
        let nrm = (-self.u.1, self.u.0);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for (t, s) in [(t0 - 1.0, s0), (t0 - 1.0, s1), (t1 + 1.0, s0), (t1 + 1.0, s1)] {
            let (x, y) = (self.origin.0 + t * self.u.0 + s * nrm.0, self.origin.1 + t * self.u.1 + s * nrm.1);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let cl = |v: f32, n: usize| (v.max(0.0) as usize).min(n.saturating_sub(1));
        (cl(x0.floor(), w), cl(y0.floor(), h), cl(x1.ceil(), w), cl(y1.ceil(), h))
    }
}

/// **B1c: measures the straight cut at a joint.** `label` is the per-pixel part
/// partition (`w × h`, `-1` for none), `matte` the character, `over` the label of
/// the part that paints over the other one at this joint (by depth, then list
/// position), `child` the label of the limb whose bone `joint` → `tail` is,
/// `parent_bone` the other part's bone.
///
/// - **Band** (the limb's profile width): per 1 px station along the bone, the
///   matte's run across it through the limb's centre line, clipped to
///   `BAND_CLIP` (1.25) × the limb's half-width either side of that line. The
///   centre line and half-width are the median extent of the limb's own pixels
///   across the bone at 0.4–0.6 of its length, where it is clear of both joints
///   (the matte's run there would take in the body a limb touches at rest).
/// - **`t_cut`**: the over part's visible profile in the band (per station the
///   extent across it, 5-station median); the first station from the joint whose
///   width reaches 85 % of the median width of the next 10 stations, clamped to
///   0.1–0.4 × the bone's length.
///
/// `None` when the bone is shorter than 4 px, the limb has no pixel in its
/// middle, or the over part has no pixel in the band past the joint.
#[allow(clippy::too_many_arguments)]
pub fn measure_joint_cut(
    label: &[i32],
    w: usize,
    h: usize,
    matte: &[bool],
    over: i32,
    child: i32,
    joint: (f32, f32),
    tail: (f32, f32),
    parent_bone: ((f32, f32), (f32, f32)),
) -> Option<JointCut> {
    measure_joint_cut_with(label, w, h, matte, over, child, joint, tail, parent_bone, false)
}

/// [`measure_joint_cut`]; with `near_when_unmeasured` (B4c: the chest over an arm
/// whose whole root the segmenter gave the arm, a front view), an over part with
/// no pixel past the joint still gets a cut, at 0.1 × the bone's length, instead
/// of none.
#[allow(clippy::too_many_arguments)]
pub fn measure_joint_cut_with(
    label: &[i32],
    w: usize,
    h: usize,
    matte: &[bool],
    over: i32,
    child: i32,
    joint: (f32, f32),
    tail: (f32, f32),
    parent_bone: ((f32, f32), (f32, f32)),
    near_when_unmeasured: bool,
) -> Option<JointCut> {
    if label.len() != w * h || matte.len() != w * h {
        return None;
    }
    let (ax, ay) = (tail.0 - joint.0, tail.1 - joint.1);
    let len = (ax * ax + ay * ay).sqrt();
    if len < 4.0 {
        return None;
    }
    let u = (ax / len, ay / len);
    let nrm = (-u.1, u.0);
    let inside = |px: f32, py: f32| -> bool {
        let (x, y) = (px.round(), py.round());
        x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h && matte[y as usize * w + x as usize]
    };
    // The matte's run across the bone at station `t`, from `s0`, within `s_min..=s_max`.
    let run = |t: f32, s0: f32, s_min: f32, s_max: f32| -> Option<(f32, f32)> {
        let at = |s: f32| inside(joint.0 + t * u.0 + s * nrm.0, joint.1 + t * u.1 + s * nrm.1);
        if !at(s0) {
            return None;
        }
        let (mut lo, mut hi) = (s0, s0);
        while lo - 1.0 >= s_min && at(lo - 1.0) {
            lo -= 1.0;
        }
        while hi + 1.0 <= s_max && at(hi + 1.0) {
            hi += 1.0;
        }
        Some((lo, hi))
    };
    // The limb's own cross-section, clear of its joints: per station the extent
    // of its pixels across the bone, then the median of those extents.
    let (k_mid0, k_mid1) = ((0.4 * len).round() as i64, (0.6 * len).round() as i64);
    let n_mid = (k_mid1 - k_mid0 + 1).max(0) as usize;
    let mut mid: Vec<Option<(f32, f32)>> = vec![None; n_mid];
    for i in (0..w * h).filter(|&i| label[i] == child) {
        let (dx, dy) = ((i % w) as f32 - joint.0, (i / w) as f32 - joint.1);
        let (t, s) = (dx * u.0 + dy * u.1, dx * nrm.0 + dy * nrm.1);
        let k = t.floor() as i64 - k_mid0;
        if k >= 0 && (k as usize) < n_mid {
            let e = &mut mid[k as usize];
            *e = Some(e.map_or((s, s), |(lo, hi)| (lo.min(s), hi.max(s))));
        }
    }
    let mut los: Vec<f32> = mid.iter().flatten().map(|e| e.0).collect();
    let mut his: Vec<f32> = mid.iter().flatten().map(|e| e.1).collect();
    if los.is_empty() {
        return None;
    }
    let (lo_m, hi_m) = (lower_median(&mut los), lower_median(&mut his));
    let c_m = 0.5 * (lo_m + hi_m);
    let hw_m = 0.5 * (hi_m - lo_m) + 0.5;
    let clip = (c_m - BAND_CLIP * hw_m, c_m + BAND_CLIP * hw_m);
    let k_lo = (-len).floor() as i64;
    let k_hi = len.ceil() as i64 + CUT_LOOKAHEAD_STATIONS + 1;
    let band: Vec<Option<(f32, f32)>> = (k_lo..=k_hi).map(|k| run(k as f32, c_m, clip.0, clip.1)).collect();
    let mut cut = JointCut { origin: joint, u, len, parent_bone, t_cut: 0.0, k_lo, band };

    // The over part's profile in the band, from the joint on.
    let n_st = (k_hi + 1) as usize;
    let mut raw_lo: Vec<Option<f32>> = vec![None; n_st];
    let mut raw_hi: Vec<Option<f32>> = vec![None; n_st];
    let (x0, y0, x1, y1) = cut.bounds(0.0, k_hi as f32, w, h);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if label[y * w + x] != over {
                continue;
            }
            let (t, s) = cut.frame(x, y);
            if t < 0.0 || !cut.in_band(t, s) {
                continue;
            }
            let k = t.floor() as usize;
            if k < n_st {
                raw_lo[k] = Some(raw_lo[k].map_or(s, |v| v.min(s)));
                raw_hi[k] = Some(raw_hi[k].map_or(s, |v| v.max(s)));
            }
        }
    }
    let half = MEDIAN_STATIONS / 2;
    let smooth = |raw: &[Option<f32>]| -> Vec<Option<f32>> {
        (0..n_st as i64)
            .map(|i| {
                raw[i as usize]?;
                let mut window: Vec<f32> =
                    ((i - half).max(0)..=(i + half).min(n_st as i64 - 1)).filter_map(|j| raw[j as usize]).collect();
                window.sort_by(f32::total_cmp);
                Some(window[(window.len() - 1) / 2])
            })
            .collect()
    };
    let (lo, hi) = (smooth(&raw_lo), smooth(&raw_hi));
    let width: Vec<f32> = (0..n_st)
        .map(|k| match (lo[k], hi[k]) {
            (Some(a), Some(b)) => b - a + 1.0,
            _ => 0.0,
        })
        .collect();
    // B4c: an over part with no pixel past the joint (a front view's chest, the
    // arm's whole root its own) may still get a cut, at the clamp's near end, so
    // the limb ends straight across and the body takes back its flank (B1d).
    let k_found = (0..=len.floor() as usize)
        .find(|&k| {
            let mut ahead: Vec<f32> =
                (k + 1..=k + CUT_LOOKAHEAD_STATIONS as usize).map(|j| width.get(j).copied().unwrap_or(0.0)).collect();
            width[k] > 0.0 && width[k] >= CUT_WIDTH_SHARE * lower_median(&mut ahead)
        })
        .or(near_when_unmeasured.then_some(0))?;
    cut.t_cut = (k_found as f32).clamp(CUT_CLAMP.0 * len, CUT_CLAMP.1 * len).round();
    Some(cut)
}

/// **B1c: applies a joint's straight cut** to the partition `label` (`w × h`):
/// within the cut's band, the child's pixels on the joint side of the cut
/// (`t < t_cut`) go to the parent and the parent's pixels on the far side
/// (`t_cut ≤ t ≤ len`, nearer the child's bone than the parent's: a parent lying
/// alongside the limb, a bent elbow's upper arm, stays the parent's) go to the
/// child, so the two parts meet on the straight line `t = t_cut`. The child's
/// pixels move from behind the joint too (`−len ≤ t < t_cut`): its visible edge is
/// then the cut whichever part paints over the other, it is extruded from there
/// to the joint when the parent covers it (B1), and no island of it is left
/// behind the joint (a bent elbow's point; at the ankle, the heel goes with the
/// shin rather than the shoe being split into heel, strip and toe).
/// `yields(x, y, |s|)` is asked before a parent pixel moves to the child: `true`
/// leaves it (a sibling joint on the same parent claims it). Returns the number
/// of pixels moved.
pub fn apply_joint_cut(
    label: &mut [i32],
    w: usize,
    h: usize,
    cut: &JointCut,
    parent: i32,
    child: i32,
    mut yields: impl FnMut(usize, usize, f32) -> bool,
) -> usize {
    if label.len() != w * h {
        return 0;
    }
    let near_from = -cut.len;
    let (x0, y0, x1, y1) = cut.bounds(-cut.len, cut.len, w, h);
    let mut moved = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let i = y * w + x;
            let l = label[i];
            if l != parent && l != child {
                continue;
            }
            let (t, s) = cut.frame(x, y);
            if t < -cut.len || t > cut.len || !cut.in_band(t, s) {
                continue;
            }
            if l == child && t < cut.t_cut && t >= near_from {
                label[i] = parent;
                moved += 1;
            } else if l == parent
                && t >= cut.t_cut
                && segment_distance((x as f32, y as f32), cut.origin, (cut.origin.0 + cut.len * cut.u.0, cut.origin.1 + cut.len * cut.u.1))
                    < segment_distance((x as f32, y as f32), cut.parent_bone.0, cut.parent_bone.1)
                && !yields(x, y, s.abs())
            {
                label[i] = child;
                moved += 1;
            }
        }
    }
    moved
}

/// **B4b-3: a round end on the part that paints over at a limb joint** (the shin
/// over the thigh at the knee). B1c's straight cut left the shin a bare straight
/// top `t_cut` below the knee and the thigh a straight bottom there: when the knee
/// bends, the two straight ends turn apart into a step on one side and a wedge
/// gap on the other. Here, within the cut's band, the over part (`child`) takes
/// everything from the joint on (`t ≥ 0`, nearer its bone than the parent's) and
/// a half-disc past the joint: centred on the band's centre at the joint, radius
/// half the band's width there. It keeps the source pixels (it paints over the
/// parent, so the rest picture is the same), and turning about the joint it
/// sweeps over its own disc: no step. The parent keeps the rest, is hidden under
/// the disc and is extruded under it from `r` above the joint
/// ([`outpaint_limb_both_ends`]). `keep_radius` (B4c, a body parent: the chest
/// under an arm): parent pixels within it of the parent's bone are never moved,
/// so the arm takes only the sleeve cap outside the torso's capsule. Returns the
/// pixels moved and `r`; `None` when the band has no run at the joint.
pub fn round_over_end(
    label: &mut [i32],
    w: usize,
    h: usize,
    cut: &JointCut,
    parent: i32,
    child: i32,
    keep_radius: Option<f32>,
) -> Option<(usize, f32)> {
    if label.len() != w * h {
        return None;
    }
    let i0 = usize::try_from(-cut.k_lo).ok()?;
    let (lo, hi) = (*cut.band.get(i0)?)?;
    let (c, r) = (0.5 * (lo + hi), 0.5 * (hi - lo) + 0.5);
    let child_end = (cut.origin.0 + cut.len * cut.u.0, cut.origin.1 + cut.len * cut.u.1);
    let (x0, y0, x1, y1) = cut.bounds(-r - 1.0, cut.len, w, h);
    let mut moved = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let i = y * w + x;
            let l = label[i];
            if l != parent && l != child {
                continue;
            }
            let (t, s) = cut.frame(x, y);
            if t < -r - 1.0 || t > cut.len || !cut.in_band(t, s) {
                continue;
            }
            let in_disc = t < 0.0 && t * t + (s - c) * (s - c) <= r * r;
            // B4c: a body parent keeps its core, the pixels within its capsule.
            let kept = keep_radius
                .is_some_and(|kr| segment_distance((x as f32, y as f32), cut.parent_bone.0, cut.parent_bone.1) <= kr);
            if l == parent
                && !kept
                && (in_disc
                    || (t >= 0.0
                        && segment_distance((x as f32, y as f32), cut.origin, child_end)
                            < segment_distance((x as f32, y as f32), cut.parent_bone.0, cut.parent_bone.1)))
            {
                label[i] = child;
                moved += 1;
            } else if l == child && t < 0.0 && !in_disc {
                label[i] = parent;
                moved += 1;
            }
        }
    }
    Some((moved, r))
}

/// **B4c: a limb's joint disc**: centred on the band's centre at the joint (image
/// coordinates), radius half the band's width there, no larger than the distance
/// from that centre to the nearest pixel outside `matte` less 1 px (`w × h`), so it
/// stays inside the silhouette whichever way the limb turns. `None` when the band
/// has no run at the joint.
pub fn joint_disc(cut: &JointCut, matte: &[bool], w: usize, h: usize) -> Option<((f32, f32), f32)> {
    let i0 = usize::try_from(-cut.k_lo).ok()?;
    let (lo, hi) = (*cut.band.get(i0)?)?;
    let (c, r) = (0.5 * (lo + hi), 0.5 * (hi - lo) + 0.5);
    let nrm = (-cut.u.1, cut.u.0);
    let centre = (cut.origin.0 + c * nrm.0, cut.origin.1 + c * nrm.1);
    let r = if matte.len() == w * h { r.min(distance_to_outside(matte, w, h, centre, r + 2.0) - 1.0) } else { r };
    (r > 0.0).then_some((centre, r))
}

/// The 8-neighbourhood.
const NEIGHBOURS_8: [(i64, i64); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

/// B4b-3: a part's pixels under this many px wide are spikes.
const SPIKE_PX: usize = 4;

/// **B4b-3: spikes.** The pixels of `part` that no `SPIKE_PX` × `SPIKE_PX` square of
/// its own pixels covers (an opening: everything under 4 px wide, the teeth a
/// comb-shaped segmenter boundary leaves along a limb) go to the other part that
/// shares the most 8-neighbours with them, when one does (a spike only bordering
/// the background stays). They show the same source pixels at rest; posed they
/// stay with the part they lie in. Returns the number of pixels moved.
pub fn remove_spikes(label: &mut [i32], w: usize, h: usize, part: i32) -> usize {
    if label.len() != w * h || w < SPIKE_PX || h < SPIKE_PX {
        return 0;
    }
    let k = SPIKE_PX;
    // Integral image of the part's pixels; `eroded[(x, y)]`: the k × k square from (x, y) is all the part's.
    let mut integral = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += (label[y * w + x] == part) as u32;
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let sum = |ig: &[u32], x0: usize, y0: usize, x1: usize, y1: usize| -> u32 {
        ig[y1 * (w + 1) + x1] + ig[y0 * (w + 1) + x0] - ig[y0 * (w + 1) + x1] - ig[y1 * (w + 1) + x0]
    };
    let mut eroded_ig = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            let full = x + k <= w && y + k <= h && sum(&integral, x, y, x + k, y + k) == (k * k) as u32;
            row += full as u32;
            eroded_ig[(y + 1) * (w + 1) + x + 1] = eroded_ig[y * (w + 1) + x + 1] + row;
        }
    }
    let spikes: Vec<usize> = (0..w * h)
        .filter(|&i| {
            if label[i] != part {
                return false;
            }
            let (x, y) = (i % w, i / w);
            // Covered when a full square starts within the k × k window ending here.
            sum(&eroded_ig, x.saturating_sub(k - 1), y.saturating_sub(k - 1), x + 1, y + 1) == 0
        })
        .collect();
    relabel_to_neighbours(label, w, h, part, &spikes, |_| true)
}

/// Passes [`relabel_to_neighbours`] makes at most (each moves the front 1 px in).
const RELABEL_PASSES: usize = 64;

/// Gives each of `pixels` (all `part`'s) to the part, `accept`ed, that shares the
/// most 8-neighbours with it (ties: the lower label), in passes that each look at
/// the labels as the pass found them (so a strip is eaten in from all its sides,
/// not run down from one end in scan order), until nothing moves; a pixel with no
/// such neighbour stays `part`'s. Returns the number of pixels moved.
fn relabel_to_neighbours(label: &mut [i32], w: usize, h: usize, part: i32, pixels: &[usize], accept: impl Fn(i32) -> bool) -> usize {
    let mut moved = 0;
    let mut pending = pixels.to_vec();
    for _ in 0..RELABEL_PASSES {
        let mut next = Vec::new();
        let mut assign: Vec<(usize, i32)> = Vec::new();
        for &i in &pending {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let mut counts: Vec<(i32, u32)> = Vec::new();
            for (dx, dy) in NEIGHBOURS_8 {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    continue;
                }
                let l = label[ny as usize * w + nx as usize];
                if l < 0 || l == part || !accept(l) {
                    continue;
                }
                match counts.iter_mut().find(|e| e.0 == l) {
                    Some(e) => e.1 += 1,
                    None => counts.push((l, 1)),
                }
            }
            match counts.iter().max_by_key(|e| (e.1, std::cmp::Reverse(e.0))) {
                Some(&(l, _)) => assign.push((i, l)),
                None => next.push(i),
            }
        }
        if assign.is_empty() {
            break;
        }
        moved += assign.len();
        for (i, l) in assign {
            label[i] = l;
        }
        pending = next;
    }
    moved
}

/// B4b-4: an object's seed colour is saturated at least this much (max − min channel).
const FOREIGN_SATURATION: i32 = 100;
/// B4b-4: a colour bin is foreign to a limb when under this share of its far pixels fall in it.
const FOREIGN_BIN_SHARE: f64 = 0.001;
/// B4b-4: ... and under this share of the body part's own pixels.
const FOREIGN_BODY_SHARE: f64 = 0.005;
/// B4b-4: an object takes the darker (outline, anti-aliased) pixels this close to its colour (px).
const FOREIGN_GROW_PX: usize = 3;
/// B4b-4: below this luminance a pixel next to an object is its outline (the limb's cloth is lighter).
const FOREIGN_OUTLINE_LUM: u32 = 170;
/// B4b-4: and the limb pixels this close to an object, outline included (px).
const FOREIGN_MARGIN_PX: usize = 4;

/// **B4b-4: an object of the body caught at a limb's root.** Where a limb paints
/// over the body part it hangs from (the near arm over the chest), the segmenter's
/// boundary can run through an object that belongs to the body: Raju's pens stand
/// in his shirt pocket and overlap the near sleeve's root, so the arm took their
/// tops, and swung down they lay across the sleeve as a dark slash. Here, near the
/// joint (`t` up to `t_cut` + a quarter of the limb), the limb's pixels whose
/// colour is strongly saturated (max − min ≥ 100: a plaid stripe is not) and foreign to the rest of the limb (its
/// 8-level-per-channel colour bin holds under 0.1 % of the limb's pixels past that
/// zone) and rare on the body part too (under 0.5 % of its pixels), with the
/// darker pixels (luminance < 170) within 3 px of them (their
/// anti-aliased outline) and a 4 px margin, form objects;
/// each object that touches the body part goes to it. The body part paints under
/// the limb, which then does not paint there, so the rest picture is the same; the
/// limb has a notch at its root under nothing. Returns the number of pixels moved.
pub fn return_foreign_objects(
    label: &mut [i32],
    w: usize,
    h: usize,
    cut: &JointCut,
    parent: i32,
    child: i32,
    source: &image::RgbaImage,
) -> usize {
    if label.len() != w * h || source.width() as usize != w || source.height() as usize != h {
        return 0;
    }
    let zone_end = cut.t_cut + 0.25 * cut.len;
    let bin = |p: [u8; 4]| ((p[0] as usize >> 5) << 6) | ((p[1] as usize >> 5) << 3) | (p[2] as usize >> 5);
    let mut hist = vec![0usize; 512];
    let mut far = 0usize;
    let mut zone: Vec<usize> = Vec::new();
    // The body part's own colours: an object foreign to the limb but common on the
    // body (a short sleeve's stains on a shirt, a sari border) is not an object.
    let mut body_hist = vec![0usize; 512];
    let mut body = 0usize;
    for i in (0..w * h).filter(|&i| label[i] == parent) {
        body_hist[bin(source.get_pixel((i % w) as u32, (i / w) as u32).0)] += 1;
        body += 1;
    }
    for i in (0..w * h).filter(|&i| label[i] == child) {
        let (t, _) = cut.frame(i % w, i / w);
        if t > zone_end {
            hist[bin(source.get_pixel((i % w) as u32, (i / w) as u32).0)] += 1;
            far += 1;
        } else {
            zone.push(i);
        }
    }
    if far == 0 || zone.is_empty() {
        return 0;
    }
    let mut object = vec![false; w * h];
    for &i in &zone {
        let p = source.get_pixel((i % w) as u32, (i / w) as u32).0;
        let (mx, mn) = (p[0].max(p[1]).max(p[2]) as i32, p[0].min(p[1]).min(p[2]) as i32);
        object[i] = mx - mn >= FOREIGN_SATURATION
            && (hist[bin(p)] as f64) < FOREIGN_BIN_SHARE * far as f64
            && (body_hist[bin(p)] as f64) < FOREIGN_BODY_SHARE * body as f64;
    }
    // The objects' outlines: dark zone pixels next to them, grown 1 px at a time.
    let in_zone: Vec<bool> = {
        let mut v = vec![false; w * h];
        for &i in &zone {
            v[i] = true;
        }
        v
    };
    for _ in 0..FOREIGN_GROW_PX {
        let grow: Vec<usize> = zone
            .iter()
            .copied()
            .filter(|&i| {
                if object[i] {
                    return false;
                }
                let p = source.get_pixel((i % w) as u32, (i / w) as u32).0;
                let lum = (299 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32) / 1000;
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                lum < FOREIGN_OUTLINE_LUM
                    && NEIGHBOURS_8.iter().any(|&(dx, dy)| {
                        let (nx, ny) = (x + dx, y + dy);
                        nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && object[ny as usize * w + nx as usize]
                    })
            })
            .collect();
        for i in grow {
            object[i] = true;
        }
    }
    // And a margin round them, whatever its colour: the trace draws a part's ink up
    // to 3 px past its boundary (`INK_PIECE_OVERLAP_PX` in `character.rs`), so a
    // limb boundary hugging the object would still draw the object's outline.
    for _ in 0..FOREIGN_MARGIN_PX {
        let grow: Vec<usize> = zone
            .iter()
            .copied()
            .filter(|&i| {
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                !object[i]
                    && NEIGHBOURS_8.iter().any(|&(dx, dy)| {
                        let (nx, ny) = (x + dx, y + dy);
                        nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && object[ny as usize * w + nx as usize]
                    })
            })
            .collect();
        for i in grow {
            object[i] = true;
        }
    }
    // Objects (8-connected) that touch the body part go to it.
    let mut seen = vec![false; w * h];
    let mut moved = 0;
    for &start in &zone {
        if !object[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut comp = vec![start];
        let mut touches = false;
        let mut k = 0;
        while k < comp.len() {
            let (x, y) = ((comp[k] % w) as i64, (comp[k] / w) as i64);
            k += 1;
            for (dx, dy) in NEIGHBOURS_8 {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if label[j] == parent {
                    touches = true;
                }
                if in_zone[j] && object[j] && !seen[j] {
                    seen[j] = true;
                    comp.push(j);
                }
            }
        }
        if touches {
            for j in comp {
                label[j] = parent;
                moved += 1;
            }
        }
    }
    moved
}

/// B4b-1: a borrower's own pixels whose run across its bone is shorter than this are slivers.
const SLIVER_RUN_PX: usize = 4;
/// B4b-1: a borrower's own component under this share of the borrowed picture is a fragment.
const FRAGMENT_SHARE: f64 = 0.15;

/// **B4b-1: a borrowing limb's slivers and fragments.** A limb that borrows its
/// counterpart's picture (B1b) shows its own visible pixels on top of it; where
/// those are thin slivers (a run across the bone `head`–`tail` under 4 px) or a
/// component (8-connected) under 15 % of `borrowed_px` (the borrowed picture's
/// size), they were drawn ragged over the whole borrowed limb when it swung away
/// (the far thigh's strips, the tongue of it the segmenter put in the middle of
/// the near trouser leg). Each goes to the part that paints over `part` (`over`)
/// and shares the most 8-neighbours with it: that part shows the same source pixel
/// at rest, and the borrowed picture is completed under it, so the far limb shows
/// whole; the counterpart `prefer` takes what touches it first. Returns the number
/// of pixels moved.
#[allow(clippy::too_many_arguments)]
pub fn drop_borrower_fragments(
    label: &mut [i32],
    w: usize,
    h: usize,
    part: i32,
    head: (f32, f32),
    tail: (f32, f32),
    borrowed_px: usize,
    prefer: i32,
    over: impl Fn(i32) -> bool,
) -> usize {
    if label.len() != w * h {
        return 0;
    }
    let (ax, ay) = (tail.0 - head.0, tail.1 - head.1);
    let len = (ax * ax + ay * ay).sqrt();
    if len < 1e-3 {
        return 0;
    }
    let nrm = (-ay / len, ax / len);
    let own = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && label[y as usize * w + x as usize] == part;
    // The run across the bone through a pixel: 1 px steps along the normal, both ways.
    let run = |x: usize, y: usize| -> usize {
        let mut n = 1;
        for sign in [-1.0f32, 1.0] {
            for k in 1..SLIVER_RUN_PX {
                let (px, py) = (x as f32 + sign * k as f32 * nrm.0, y as f32 + sign * k as f32 * nrm.1);
                if !own(px.round() as i64, py.round() as i64) {
                    break;
                }
                n += 1;
            }
        }
        n
    };
    let mut drop = vec![false; w * h];
    for i in (0..w * h).filter(|&i| label[i] == part) {
        drop[i] = run(i % w, i / w) < SLIVER_RUN_PX;
    }
    // Components of what is left; the small ones are fragments.
    let limit = FRAGMENT_SHARE * borrowed_px as f64;
    let mut seen = vec![false; w * h];
    for start in 0..w * h {
        if seen[start] || label[start] != part || drop[start] {
            continue;
        }
        seen[start] = true;
        let mut comp = vec![start];
        let mut k = 0;
        while k < comp.len() {
            let (x, y) = ((comp[k] % w) as i64, (comp[k] / w) as i64);
            k += 1;
            for (dx, dy) in NEIGHBOURS_8 {
                let (nx, ny) = (x + dx, y + dy);
                if !own(nx, ny) {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if !seen[j] && !drop[j] {
                    seen[j] = true;
                    comp.push(j);
                }
            }
        }
        if (comp.len() as f64) < limit {
            for j in comp {
                drop[j] = true;
            }
        }
    }
    // Only to the counterpart (`prefer`: the near limb the fragment lies in), so a
    // strip along it goes with it whole; a fragment that does not touch it (one
    // beside the hips or the near shoe) stays: handing it to those parts changed
    // their traced ink at rest.
    let pixels: Vec<usize> = (0..w * h).filter(|&i| drop[i]).collect();
    relabel_to_neighbours(label, w, h, part, &pixels, |l| l == prefer && over(l))
}

/// **B1d: a body part's prior** (Chest, Hips, Head): the region it may take back
/// from a limb that paints behind it. Its completion capsule (`radius` round the
/// bone `head`–`tail`) and, per 1 px station along that bone, the span across it
/// of its own pixels (`label == part`; the extents smoothed by a 5-station
/// median), so the strips and holes the segmenter left inside its rows (a thigh's
/// seam line running up through the hips) are inside it, and nothing past its
/// own outline is.
pub fn body_prior(label: &[i32], w: usize, h: usize, part: i32, head: (f32, f32), tail: (f32, f32), radius: f32) -> Vec<bool> {
    let mut prior = vec![false; w * h];
    if label.len() != w * h {
        return prior;
    }
    let (ax, ay) = (tail.0 - head.0, tail.1 - head.1);
    let len = (ax * ax + ay * ay).sqrt();
    if len < 1e-3 {
        return prior;
    }
    let u = (ax / len, ay / len);
    let nrm = (-u.1, u.0);
    let mut own: Vec<(i64, f32)> = Vec::new();
    for i in (0..w * h).filter(|&i| label[i] == part) {
        let (dx, dy) = ((i % w) as f32 - head.0, (i / w) as f32 - head.1);
        own.push(((dx * u.0 + dy * u.1).floor() as i64, dx * nrm.0 + dy * nrm.1));
    }
    let (k0, k1) = (own.iter().map(|p| p.0).min().unwrap_or(0), own.iter().map(|p| p.0).max().unwrap_or(-1));
    let n = (k1 - k0 + 1).max(0) as usize;
    let mut raw: Vec<Option<(f32, f32)>> = vec![None; n];
    for &(k, s) in &own {
        let e = &mut raw[(k - k0) as usize];
        *e = Some(e.map_or((s, s), |(lo, hi)| (lo.min(s), hi.max(s))));
    }
    let half = MEDIAN_STATIONS / 2;
    let span: Vec<Option<(f32, f32)>> = (0..n as i64)
        .map(|i| {
            raw[i as usize]?;
            let window: Vec<(f32, f32)> =
                ((i - half).max(0)..=(i + half).min(n as i64 - 1)).filter_map(|j| raw[j as usize]).collect();
            let mut los: Vec<f32> = window.iter().map(|e| e.0).collect();
            let mut his: Vec<f32> = window.iter().map(|e| e.1).collect();
            los.sort_by(f32::total_cmp);
            his.sort_by(f32::total_cmp);
            Some((los[(los.len() - 1) / 2], his[his.len() / 2]))
        })
        .collect();
    for (i, p) in prior.iter_mut().enumerate() {
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        if segment_distance((x, y), head, tail) <= radius {
            *p = true;
            continue;
        }
        let (dx, dy) = (x - head.0, y - head.1);
        let k = (dx * u.0 + dy * u.1).floor() as i64 - k0;
        if k >= 0 && (k as usize) < n {
            let s = dx * nrm.0 + dy * nrm.1;
            *p = span[k as usize].is_some_and(|(lo, hi)| s >= lo && s <= hi);
        }
    }
    prior
}

/// **B1d: a body part takes back the joint side of a limb behind it.** Every pixel
/// of `child` (a limb that paints behind `parent`, a Chest / Hips / Head that
/// paints over it at this joint) on the joint side of the cut (`t < t_cut` along
/// the child bone) and inside the parent's `prior` ([`body_prior`]) goes to the
/// parent: B1c's band only reached across the limb's own width, so the strips
/// beside it (the hips' seam lines, the thigh's old lobe under the belt) stayed
/// the limb's, swung out with it and left the body notched. The parent now shows
/// those source pixels (it paints over the limb, so the rest picture is the
/// same), and the limb completes under them. Returns the number of pixels moved.
pub fn take_back_joint_side(label: &mut [i32], w: usize, h: usize, cut: &JointCut, parent: i32, child: i32, prior: &[bool]) -> usize {
    if label.len() != w * h || prior.len() != w * h {
        return 0;
    }
    let mut moved = 0;
    for i in 0..w * h {
        if label[i] != child || !prior[i] {
            continue;
        }
        let (t, _) = cut.frame(i % w, i / w);
        if t < cut.t_cut {
            label[i] = parent;
            moved += 1;
        }
    }
    moved
}

/// B4c: how far (× the limb's half-width at the joint) a body part takes the limb's flank back.
const FLANK_REACH: f32 = 2.0;

/// **B4c: a body part takes back the limb's flank** on the joint side of the cut:
/// the limb's pixels outside its band (`t < t_cut`, beside the limb's own width)
/// within `FLANK_REACH` × its half-width of the joint. A front view's shirt hem
/// hangs past the hip joint beside the thigh: the segmenter gave its corners to
/// the thighs, and they swung out of the shirt with the legs. Returns the number
/// of pixels moved.
pub fn take_back_flank(label: &mut [i32], w: usize, h: usize, cut: &JointCut, parent: i32, child: i32) -> usize {
    if label.len() != w * h {
        return 0;
    }
    let Some(reach) = usize::try_from(-cut.k_lo)
        .ok()
        .and_then(|i0| *cut.band.get(i0)?)
        .map(|(lo, hi)| FLANK_REACH * 0.5 * (hi - lo + 1.0))
    else {
        return 0;
    };
    let mut moved = 0;
    for i in 0..w * h {
        if label[i] != child {
            continue;
        }
        let (t, s) = cut.frame(i % w, i / w);
        if t < cut.t_cut && !cut.in_band(t, s) && t * t + s * s <= reach * reach {
            label[i] = parent;
            moved += 1;
        }
    }
    moved
}

/// B1b: a limb borrows its counterpart's picture when it owns less than this
/// share of the counterpart's pixel count.
const BORROW_SHARE: f64 = 0.2;

/// **B1b**: whether a limb part owning `own_px` pixels is (almost) hidden next to
/// its L/R counterpart owning `counterpart_px`: under 20 % of it.
pub fn borrows_counterpart(own_px: usize, counterpart_px: usize) -> bool {
    (own_px as f64) < BORROW_SHARE * counterpart_px as f64
}

/// B1b: once one slot of a side borrows, that side's other slots borrow too
/// when they own less than this share of their counterpart's pixels.
const CHAIN_BORROW_SHARE: f64 = 0.4;

/// **B1b**, along the chain: whether a slot on a side that already borrows is
/// still mostly hidden (under 40 % of its counterpart).
pub fn chain_borrows(own_px: usize, counterpart_px: usize) -> bool {
    (own_px as f64) < CHAIN_BORROW_SHARE * counterpart_px as f64
}

/// **B1b: a hidden limb borrows its counterpart's picture.** Returns the part's
/// straight RGBA (`w × h`) and the borrowed pixels: its own visible pixels
/// (`own_mask > 128`, colours from `source`) stay on top; every other `allowed`
/// pixel (hidden under a part that paints over this one, inside the matte) takes
/// the counterpart's completed picture `counterpart_rgba` at the point the
/// similarity transform of this bone onto the counterpart's (head → head,
/// tail → tail, no mirroring) maps it to, where that picture is opaque
/// (alpha > 128; nearest pixel).
pub fn borrow_counterpart(
    own_mask: &[u8],
    source: &image::RgbaImage,
    allowed: &[bool],
    counterpart_rgba: &[u8],
    own_bone: ((f32, f32), (f32, f32)),
    counterpart_bone: ((f32, f32), (f32, f32)),
) -> (Vec<u8>, Vec<bool>) {
    let (w, h) = (source.width() as usize, source.height() as usize);
    let n = w * h;
    let mut rgba = vec![0u8; n * 4];
    let mut borrowed = vec![false; n];
    if own_mask.len() != n || allowed.len() != n || counterpart_rgba.len() != n * 4 {
        return (rgba, borrowed);
    }
    for i in 0..n {
        if own_mask[i] > 128 {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&source.get_pixel((i % w) as u32, (i / w) as u32).0);
        }
    }
    let ((oh, ot), (ch, ct)) = (own_bone, counterpart_bone);
    let lo = ((ot.0 - oh.0).powi(2) + (ot.1 - oh.1).powi(2)).sqrt();
    let lc = ((ct.0 - ch.0).powi(2) + (ct.1 - ch.1).powi(2)).sqrt();
    if lo < 1e-3 || lc < 1e-3 {
        return (rgba, borrowed);
    }
    let uo = ((ot.0 - oh.0) / lo, (ot.1 - oh.1) / lo);
    let uc = ((ct.0 - ch.0) / lc, (ct.1 - ch.1) / lc);
    let scale = lc / lo;
    for i in 0..n {
        if own_mask[i] > 128 || !allowed[i] {
            continue;
        }
        let (dx, dy) = ((i % w) as f32 - oh.0, (i / w) as f32 - oh.1);
        let (t, s) = ((dx * uo.0 + dy * uo.1) * scale, (-dx * uo.1 + dy * uo.0) * scale);
        let (qx, qy) = ((ch.0 + t * uc.0 - s * uc.1).round(), (ch.1 + t * uc.1 + s * uc.0).round());
        if qx < 0.0 || qy < 0.0 || qx as usize >= w || qy as usize >= h {
            continue;
        }
        let q = qy as usize * w + qx as usize;
        if counterpart_rgba[q * 4 + 3] > 128 {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&counterpart_rgba[q * 4..q * 4 + 4]);
            borrowed[i] = true;
        }
    }
    (rgba, borrowed)
}

/// **B1, the parts that are not limbs**: paints every `hole` pixel of `rgba`
/// (straight RGBA, `w × h`) opaque in the colour of the part's nearest visible
/// pixel (alpha > 128): 8-connected distance layers from the visible pixels, each
/// pixel taking the colour of the first one to reach it, passed on unchanged
/// (never a blend), as the tracer's under-painting does. `fallback` (the part's
/// dominant colour) where the part has no visible pixel.
pub fn nearest_visible_fill(rgba: &mut [u8], w: u32, h: u32, hole: &[bool], fallback: [u8; 3]) {
    let (w, h) = (w as usize, h as usize);
    let n = w * h;
    if rgba.len() != n * 4 || hole.len() != n {
        return;
    }
    let targets: Vec<bool> = (0..n).map(|i| hole[i] && rgba[i * 4 + 3] <= 128).collect();
    let mut remaining = targets.iter().filter(|&&t| t).count();
    if remaining == 0 {
        return;
    }
    const NONE: u32 = u32::MAX;
    let mut donor = vec![NONE; n];
    let mut seen = vec![false; n];
    let mut frontier: Vec<usize> = Vec::new();
    for i in 0..n {
        if rgba[i * 4 + 3] > 128 {
            seen[i] = true;
            donor[i] = i as u32;
            frontier.push(i);
        }
    }
    const NEIGHBOURS: [(i64, i64); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
    while !frontier.is_empty() && remaining > 0 {
        let mut next: Vec<usize> = Vec::new();
        for &i in &frontier {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for (dx, dy) in NEIGHBOURS {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if seen[j] {
                    continue;
                }
                seen[j] = true;
                donor[j] = donor[i];
                if targets[j] {
                    remaining -= 1;
                }
                next.push(j);
            }
        }
        frontier = next;
    }
    for i in 0..n {
        if !targets[i] {
            continue;
        }
        let rgb = match donor[i] {
            NONE => fallback,
            d => {
                let d = d as usize * 4;
                [rgba[d], rgba[d + 1], rgba[d + 2]]
            }
        };
        rgba[i * 4..i * 4 + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 200;
    const H: u32 = 100;
    const OUTLINE: [u8; 3] = [20, 20, 25];
    const BAND_A: [u8; 3] = [200, 60, 60];
    const BAND_B: [u8; 3] = [60, 60, 200];

    /// A horizontal limb on rows 40..60, its head (parent joint) at x 50 and its
    /// tail at x 170: a 3 px dark outline on its top and bottom rows and 6 px
    /// colour bands alternating along its length between them.
    fn striped_limb() -> image::RgbaImage {
        let mut img = image::RgbaImage::new(W, H);
        for y in 40..60u32 {
            for x in 50..=170u32 {
                let c = if y < 43 || y >= 57 {
                    OUTLINE
                } else if (x / 6) % 2 == 0 {
                    BAND_A
                } else {
                    BAND_B
                };
                img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
        img
    }

    #[test]
    fn a_cut_limb_continues_its_stripes_and_outline_to_the_joint() {
        let source = striped_limb();
        let (w, h) = (W as usize, H as usize);
        let (head, tail) = ((50.0f32, 49.5f32), (170.0f32, 49.5f32));
        // The occluder covers everything left of x 90: the limb's head-side 40 px
        // are hidden. The matte is the box x 20..180, y 30..70, less a 5 px slot
        // (x 70..75) the tube crosses, 20 px from the joint, so the round end keeps
        // its full radius (B1c: `min(hw0, 20 − 1)`).
        let in_matte = |x: usize, y: usize| (20..180).contains(&x) && (30..70).contains(&y) && !(70..75).contains(&x);
        let matte: Vec<bool> = (0..w * h).map(|i| in_matte(i % w, i / w)).collect();
        let mut visible = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 90..w {
                let p = source.get_pixel(x as u32, y as u32).0;
                if p[3] > 0 {
                    visible[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&p);
                }
            }
        }
        let allowed: Vec<bool> =
            (0..w * h).map(|i| in_matte(i % w, i / w) && i % w < 90 && visible[i * 4 + 3] == 0).collect();

        let out = outpaint_limb(&visible, &allowed, &matte, head, tail, &source).expect("the limb has visible pixels");
        let px = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * w + x) * 4;
            [out.rgba[i], out.rgba[i + 1], out.rgba[i + 2], out.rgba[i + 3]]
        };

        // Every hidden tube pixel within the limb's profile has the colour of the
        // station at the cut (x 90 + 2): the stripes and the outline rows continue
        // to the joint.
        for y in 40..60usize {
            let want = source.get_pixel(92, y as u32).0;
            for x in 50..90usize {
                if !in_matte(x, y) {
                    continue;
                }
                assert_eq!(px(x, y), [want[0], want[1], want[2], 255], "pixel ({x}, {y}) continues the station at the cut");
                assert!(out.extruded[y * w + x], "pixel ({x}, {y}) is a tube pixel");
            }
        }
        for y in (40..43usize).chain(57..60) {
            for x in (50..90usize).filter(|&x| in_matte(x, y)) {
                assert_eq!(&px(x, y)[..3], &OUTLINE, "the outline row {y} continues to the joint at x {x}");
            }
        }

        // Outside the tube and outside the matte, nothing.
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if !in_matte(x, y) && x < 90 {
                    assert_eq!(out.rgba[i * 4 + 3], 0, "({x}, {y}) is outside the matte and stays transparent");
                }
                if x < 90 && (y < 40 || y >= 60) {
                    assert_eq!(out.rgba[i * 4 + 3], 0, "({x}, {y}) is outside the tube and stays transparent");
                }
                assert_eq!(out.extruded[i], x < 90 && out.rgba[i * 4 + 3] == 255, "extruded marks exactly the tube pixels");
            }
        }

        // The end is round: nothing beyond head - r_cap (r_cap = 9.5, half the
        // 19 px centre-to-centre width), and the rows narrow towards the tip.
        let r_cap = 9.5f32;
        for y in 0..h {
            for x in 0..50usize {
                if out.rgba[(y * w + x) * 4 + 3] > 0 {
                    assert!(x as f32 >= head.0 - r_cap, "({x}, {y}) lies beyond head - r_cap");
                    let (t, s) = (x as f32 - head.0, y as f32 - head.1);
                    assert!(t * t + s * s <= r_cap * r_cap + 1e-3, "({x}, {y}) lies outside the round end");
                }
            }
        }
        assert_eq!(px(42, 40)[3], 0, "the cap's corner is cut off (not square)");
        assert_eq!(px(42, 49)[3], 255, "the cap's middle reaches past the joint");
        assert_eq!(px(41, 49)[3], 255, "the tip of the round end is painted");
        for y in 0..h {
            if px(41, y)[3] > 0 {
                assert_ne!(&px(41, y)[..3], &OUTLINE, "no outline is drawn across the round end (row {y})");
            }
        }
    }

    #[test]
    fn a_part_cut_at_its_tail_extrudes_towards_the_tail() {
        let source = striped_limb();
        let (w, h) = (W as usize, H as usize);
        // Visible from the head (x 50) to x 130; the tail-side 40 px are hidden.
        let mut visible = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..130usize {
                let p = source.get_pixel(x as u32, y as u32).0;
                visible[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&p);
            }
        }
        let allowed: Vec<bool> = (0..w * h).map(|i| i % w >= 130 && (30..70).contains(&(i / w))).collect();
        let matte: Vec<bool> = (0..w * h).map(|i| allowed[i] || visible[i * 4 + 3] > 0).collect();
        let out = outpaint_limb(&visible, &allowed, &matte, (50.0, 49.5), (170.0, 49.5), &source).expect("visible");
        let want = source.get_pixel(127, 50).0;
        assert_eq!(&out.rgba[(50 * w + 165) * 4..(50 * w + 165) * 4 + 3], &want[..3]);
        assert!(out.extruded[50 * w + 170] && !out.extruded[50 * w + 125]);
    }

    #[test]
    fn a_non_limb_hole_takes_the_nearest_visible_colour() {
        let (w, h) = (20u32, 1u32);
        let mut rgba = vec![0u8; 20 * 4];
        rgba[0..4].copy_from_slice(&[10, 20, 30, 255]);
        rgba[19 * 4..20 * 4].copy_from_slice(&[200, 100, 50, 255]);
        let hole: Vec<bool> = (0..20).map(|x| (1..19).contains(&x)).collect();
        nearest_visible_fill(&mut rgba, w, h, &hole, [1, 2, 3]);
        assert_eq!(&rgba[4..8], &[10, 20, 30, 255]);
        assert_eq!(&rgba[18 * 4..19 * 4], &[200, 100, 50, 255]);
        assert!(is_limb_bone("shoulder l") && is_limb_bone("Ankle R") && !is_limb_bone("Chest") && !is_limb_bone("Hips"));
    }

    /// B1c: a limb (rows 40..=60, bone x 50 → 170 on row 50) under a parent block
    /// (x 44..=80, rows 34..=66) that paints over it; the segmenter's boundary
    /// between them is a saw-tooth (the limb starts at x 66, 71, 76, 81 or 86 by
    /// row). After the cut and the extrusion the boundary is the straight line
    /// x = 50 + t_cut, and the tube runs from it to the joint, tapering to a round
    /// end that fits inside the matte.
    #[test]
    fn a_saw_tooth_joint_boundary_becomes_a_straight_cut_and_a_round_end() {
        let (w, h) = (W as usize, H as usize);
        let in_limb = |x: usize, y: usize| (50..=170).contains(&x) && (40..=60).contains(&y);
        let in_block = |x: usize, y: usize| (44..=80).contains(&x) && (34..=66).contains(&y);
        let matte: Vec<bool> = (0..w * h).map(|i| in_limb(i % w, i / w) || in_block(i % w, i / w)).collect();
        let x_start = |y: usize| 66 + ((y - 40) % 5) * 5;
        const PARENT: i32 = 0;
        const CHILD: i32 = 1;
        let mut label: Vec<i32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if in_limb(x, y) && x >= x_start(y) {
                    CHILD
                } else if matte[i] {
                    PARENT
                } else {
                    -1
                }
            })
            .collect();
        let mut source = image::RgbaImage::new(W, H);
        for i in (0..w * h).filter(|&i| matte[i]) {
            let (x, y) = (i % w, i / w);
            let c = if !in_limb(x, y) {
                [150, 140, 90]
            } else if y < 43 || y > 57 {
                OUTLINE
            } else if (x / 6) % 2 == 0 {
                BAND_A
            } else {
                BAND_B
            };
            source.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
        }
        let (head, tail) = ((50.0f32, 50.0f32), (170.0f32, 50.0f32));

        // The parent paints over the limb: its profile along the limb's bone is
        // full from the joint on, so the cut is the clamp's 0.1 × 120 px.
        let cut = measure_joint_cut(&label, w, h, &matte, PARENT, CHILD, head, tail, ((20.0, 50.0), head)).expect("a cut");
        assert_eq!(cut.t_cut, 12.0);
        let moved = apply_joint_cut(&mut label, w, h, &cut, PARENT, CHILD, |_, _, _| false);
        assert!(moved > 0);
        let x_cut = 62usize;
        for y in 40..=60usize {
            for x in x_cut..=170 {
                assert_eq!(label[y * w + x], CHILD, "({x}, {y}) past the cut is the limb's: the full profile");
            }
            assert_eq!(label[y * w + x_cut - 1], PARENT, "({}, {y}) short of the cut is the parent's", x_cut - 1);
        }
        assert!((0..w * h).all(|i| i % w >= x_cut || label[i] != CHILD), "no limb pixel short of the cut");

        // The extrusion from the straight cut.
        let mut visible = vec![0u8; w * h * 4];
        for i in (0..w * h).filter(|&i| label[i] == CHILD) {
            visible[i * 4..i * 4 + 4].copy_from_slice(&source.get_pixel((i % w) as u32, (i / w) as u32).0);
        }
        let allowed: Vec<bool> = (0..w * h).map(|i| matte[i] && label[i] == PARENT).collect();
        let out = outpaint_limb(&visible, &allowed, &matte, head, tail, &source).expect("the limb has visible pixels");
        let painted = |x: usize, y: usize| out.rgba[(y * w + x) * 4 + 3] > 0;
        // hw0 = 13 (rows 37..=63 at the cut: the band is the limb's own 21 px
        // cross-section at its middle widened to 1.25×), the joint is 7 px from the matte's
        // edge (x 43), so r_end = min(13, 7 − 1) = 6.
        let (hw0, r_end, t_cut) = (13.0f32, 6.0f32, 12.0f32);
        for y in 0..h {
            for x in 0..x_cut {
                if !painted(x, y) {
                    continue;
                }
                assert!(out.extruded[y * w + x], "({x}, {y}) short of the cut is a tube pixel");
                let (t, s) = (x as f32 - head.0, y as f32 - head.1);
                if t >= 0.0 {
                    let hw = r_end + (hw0 - r_end) * t / t_cut;
                    assert!(s.abs() <= hw + 1e-3, "({x}, {y}) lies inside the tapered tube (hw {hw})");
                } else {
                    assert!(t * t + s * s <= r_end * r_end + 1e-3, "({x}, {y}) lies inside the round end");
                }
            }
        }
        assert!(out.extruded[50 * w + 50] && out.extruded[50 * w + 44], "the tube reaches the joint and its tip");
        assert!(painted(50, 44) && painted(50, 56) && !painted(50, 43) && !painted(50, 57), "half-width r_end at the joint");
        assert!(painted(61, 38) && painted(61, 62) && !painted(61, 37) && !painted(61, 63), "the taper next to the cut");
        assert!(painted(47, 50) && !painted(45, 45), "the end is round, not square");
    }

    /// B1b: two parallel thighs in a profile view; the far one (Thigh L, listed
    /// first, depth 60) owns a 2 px sliver of the leg, the near one (Thigh R, depth
    /// 50) the rest. Thigh L borrows Thigh R's picture, moved behind it; in a front
    /// view (both legs fully visible) nothing is borrowed.
    #[test]
    fn a_hidden_limb_borrows_its_counterpart() {
        use crate::character::{complete_parts, Bone, FaceFeatures, PartMask, PartMasks, Skeleton};
        let (w, h) = (200usize, 200usize);
        let bone = |name: &str, x: f32| Bone {
            name: name.into(),
            head: (x, 40.0),
            tail: (x, 160.0),
            confidence: 1.0,
            length: 120.0,
        };
        let run = |l_cols: std::ops::RangeInclusive<usize>, r_cols: std::ops::RangeInclusive<usize>, l_x: f32, r_x: f32| {
            let mut source = image::RgbaImage::new(w as u32, h as u32);
            let mut matte = image::GrayImage::new(w as u32, h as u32);
            let (mut l_mask, mut r_mask) = (vec![0u8; w * h], vec![0u8; w * h]);
            for y in 40..=160usize {
                for x in l_cols.clone().chain(r_cols.clone()) {
                    let left = l_cols.contains(&x);
                    let c = if left { [40, 160, 60] } else if (y / 8) % 2 == 0 { [200, 60, 60] } else { [60, 60, 200] };
                    source.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
                    matte.put_pixel(x as u32, y as u32, image::Luma([255]));
                    if left {
                        l_mask[y * w + x] = 255;
                    } else {
                        r_mask[y * w + x] = 255;
                    }
                }
            }
            let parts = PartMasks {
                width: w as u32,
                height: h as u32,
                parts: vec![
                    PartMask { name: "Thigh L".into(), depth: 60, mask: l_mask },
                    PartMask { name: "Thigh R".into(), depth: 50, mask: r_mask },
                ],
                props: Vec::new(),
            };
            let skel = Skeleton {
                joints: Vec::new(),
                bones: vec![bone("Thigh L", l_x), bone("Thigh R", r_x)],
                face: FaceFeatures::default(),
                character_height: 160.0,
            };
            let completed = complete_parts(&source, &matte, &skel, &parts).expect("completed");
            (source, completed)
        };

        // Profile: Thigh L owns x 114..=115, Thigh R x 85..=113 (2 / 29 columns).
        let (source, completed) = run(114..=115, 85..=113, 104.0, 100.0);
        let far = completed.get("Thigh L").expect("Thigh L");
        let near = completed.get("Thigh R").expect("Thigh R");
        assert_eq!(far.borrowed_from.as_deref(), Some("Thigh R"));
        assert_eq!(near.borrowed_from, None);
        assert_eq!(far.depth, near.depth - 1, "the borrowing leg is moved behind the near one");
        let px = |rgba: &[u8], x: usize, y: usize| -> [u8; 4] {
            let i = (y * w + x) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        // Bone to bone is a 4 px shift: where the near leg hides it, the far leg's
        // picture is the near leg's picture 4 px to the left.
        for y in 40..=160usize {
            for x in 89..=113usize {
                assert_eq!(px(&far.rgba, x, y), px(&near.rgba, x - 4, y), "({x}, {y}) is the near leg's mapped picture");
                assert!(far.extruded[y * w + x], "({x}, {y}) is marked as completion (traced with no reach gate)");
            }
            // B4b-1 (P8-B): its own 2 px sliver goes to the near leg, which shows the
            // source there; under it the far leg shows its borrowed picture whole.
            for x in 114..=115usize {
                assert_eq!(px(&near.rgba, x, y), source.get_pixel(x as u32, y as u32).0, "its sliver is the near leg's now");
                assert_eq!(px(&far.rgba, x, y), px(&near.rgba, x - 4, y), "({x}, {y}) is the near leg's mapped picture");
            }
        }

        // Front view: each leg fully visible, nothing borrowed, nothing moved.
        let (_, completed) = run(45..=75, 125..=155, 60.0, 140.0);
        for name in ["Thigh L", "Thigh R"] {
            assert_eq!(completed.get(name).expect(name).borrowed_from, None, "{name} borrows nothing in a front view");
        }
        assert_eq!(completed.get("Thigh L").map(|p| p.depth), Some(60));
        assert_eq!(completed.get("Thigh R").map(|p| p.depth), Some(50));
    }
}
