//! The Magic Wand: a click on a picture becomes a path round what it hit.
//!
//! # Why this returns a path and not a mask
//!
//! Photoshop's wand makes a *selection*: a mask of pixels, living beside the
//! image, which the next operation consumes. Nothing here has anywhere to keep
//! such a thing, and it would be the wrong shape if it did — a bitmap in this
//! document is a **shape filled with an image**, so the thing that can remove
//! the sky from a photograph is a path, cut out of that shape by the same
//! boolean the Lasso and the Eraser use.
//!
//! So the wand's job is: pixels in, **path out**. Everything downstream then
//! works on it without knowing a bitmap was involved — it can be nudged with
//! Subselection, tweened, used as a mask, or converted to a symbol and reused,
//! none of which a pixel mask could ever be.
//!
//! # The three steps
//!
//! 1. **Flood** from the clicked pixel, taking everything within tolerance.
//! 2. **Trace** the boundary of what was taken, as unit-length lattice edges
//!    chained into closed loops. Holes come out wound the opposite way, so a
//!    non-zero fill leaves them as holes without any extra bookkeeping.
//! 3. **Simplify**, because the honest answer — one segment per pixel edge —
//!    is a hundred thousand points for a sky, and every boolean afterwards
//!    would pay for it.

use std::collections::HashMap;

use buzz_geom::{Affine, BezPath, Point};

use crate::image::{ImageAsset, ImageFill};

/// How the wand decides what it caught.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WandOptions {
    /// How far a pixel may differ from the one clicked and still be taken.
    ///
    /// `0.0` takes only exactly equal pixels; `1.0` takes everything. The
    /// default matches Photoshop's own default of 32 in its 0–255 scale, which
    /// is the number most people have in their fingers.
    pub tolerance: f64,
    /// Take only pixels reachable from the click without leaving the region.
    ///
    /// Off is Photoshop's "Contiguous" unticked: every matching pixel in the
    /// whole picture, however far away — the way to catch all the sky when a
    /// tree divides it.
    pub contiguous: bool,
    /// How far the traced path may stray from the true pixel boundary, in
    /// pixels.
    ///
    /// A stair-step boundary at pixel resolution is exact and useless: a sky in
    /// a 4K photograph traces to something like a hundred thousand segments,
    /// and every boolean, every hit test and every redraw afterwards pays for
    /// all of them. One pixel of slack is invisible at any sane zoom and takes
    /// a hundredfold off the count.
    pub simplify: f64,
    /// **Stop at drawn lines and edges.** A pixel on a strong edge — an
    /// outline, the rim of a leaf against the sky — can be taken, so no fringe
    /// is left behind, but the region does not grow through it. Colours are
    /// also judged against the picture's own contrast (a night scene is all
    /// dark, and an absolute tolerance took the road, the trees and the sky
    /// together), and a gentle gradient — a sky lightening towards the
    /// horizon — is followed further than the tolerance alone would go.
    pub stop_at_lines: bool,
    /// **Catch the gaps**: with Contiguous on, the same colour in pockets close
    /// by — sky between the fronds of a palm, ground between blades of grass —
    /// is taken with the region rather than left behind.
    pub catch_gaps: bool,
    /// **Smooth edges**: specks left inside filled, one-pixel spikes and dents
    /// shaved off, and the outline rounded, so nothing sticks out or stays
    /// behind.
    pub smooth: bool,
}

impl Default for WandOptions {
    fn default() -> Self {
        Self {
            tolerance: 32.0 / 255.0,
            contiguous: true,
            simplify: 1.0,
            stop_at_lines: true,
            catch_gaps: true,
            smooth: true,
        }
    }
}

impl WandOptions {
    /// The plain wand: the colour clicked, the tolerance, and nothing else —
    /// Photoshop's, pixel for pixel.
    pub fn plain(tolerance: f64, contiguous: bool) -> Self {
        Self { tolerance, contiguous, simplify: 1.0, stop_at_lines: false, catch_gaps: false, smooth: false }
    }
}

/// However far simplification has to go, a region never exceeds this many
/// points.
///
/// A guarantee rather than a hope. Tolerance and image alike are the user's,
/// and a noisy photograph at a high tolerance can trace a boundary that no
/// fixed epsilon tames — grain makes every pixel a corner. Rather than hand the
/// rest of the editor a path it will choke on, the epsilon is doubled until the
/// path fits. The result is coarser; it is still a path, and the editor stays
/// responsive.
const MAX_REGION_POINTS: usize = 20_000;

/// Loops smaller than this are speckle, not artwork.
const MIN_LOOP_AREA_PX: f64 = 2.0;

/// Which pixels the wand took.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    bits: Vec<bool>,
    count: usize,
}

impl Mask {
    pub fn get(&self, x: i64, y: i64) -> bool {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return false;
        }
        self.bits[(y as usize) * self.width as usize + x as usize]
    }

    /// How many pixels were taken.
    pub fn count(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Do two pixels count as the same colour?
///
/// # Why alpha is judged separately
///
/// A straight-alpha pixel that is fully transparent has no meaningful colour —
/// PNG encoders routinely leave black, white or whatever was there before under
/// a transparent pixel. Comparing those channels would make two invisible
/// pixels differ, and the wand would refuse to spread across the transparent
/// part of a cut-out. So the colour difference is scaled by how visible the
/// *less* visible of the two is, and the alpha difference is judged on its own.
fn alike(a: [u8; 4], b: [u8; 4], tolerance_255: f64) -> bool {
    if f64::from(a[3].abs_diff(b[3])) > tolerance_255 {
        return false;
    }
    let visible = f64::from(a[3].min(b[3])) / 255.0;
    let dc = f64::from(
        a[0].abs_diff(b[0])
            .max(a[1].abs_diff(b[1]))
            .max(a[2].abs_diff(b[2])),
    );
    dc * visible <= tolerance_255
}

/// **The picture in CIE Lab**, the colour space built so that equal
/// distances look equally different: L (0–100) stored ×2.5, a and b offset
/// by 128, one byte each. Worked out once per click, in parallel. Measured in
/// sRGB a night scene is all within a few dozen levels — the road, a dark-red
/// door and the black outlines alike — and the old tolerance took the lot;
/// in Lab the door is plainly another colour and the outline plainly darker.
struct LabPicture {
    w: usize,
    h: usize,
    lab: Vec<[u8; 3]>,
    alpha: Vec<u8>,
    /// Every difference is multiplied by this: see [`LabPicture::contrast`].
    scale: f32,
}

impl LabPicture {
    fn new(asset: &ImageAsset) -> Self {
        use rayon::prelude::*;
        let (w, h) = (asset.width as usize, asset.height as usize);
        let linear: Vec<f32> = (0..256)
            .map(|v| {
                let c = v as f32 / 255.0;
                if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            })
            .collect();
        let f = |t: f32| if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
        let pixels = &asset.pixels;
        let n = w * h;
        let lab: Vec<[u8; 3]> = (0..n)
            .into_par_iter()
            .map(|i| {
                let Some(p) = pixels.get(i * 4..i * 4 + 4) else { return [0, 128, 128] };
                let (r, g, b) = (linear[p[0] as usize], linear[p[1] as usize], linear[p[2] as usize]);
                let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
                let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
                let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
                let (fx, fy, fz) = (f(x), f(y), f(z));
                let l = 116.0 * fy - 16.0;
                let a = 500.0 * (fx - fy);
                let bb = 200.0 * (fy - fz);
                [(l * 2.5).round().clamp(0.0, 255.0) as u8, (a + 128.0).round().clamp(0.0, 255.0) as u8, (bb + 128.0).round().clamp(0.0, 255.0) as u8]
            })
            .collect();
        let alpha: Vec<u8> = (0..n).map(|i| pixels.get(i * 4 + 3).copied().unwrap_or(0)).collect();
        let mut pic = Self { w, h, lab, alpha, scale: 1.0 };
        pic.scale = pic.contrast();
        pic
    }

    /// **The picture's own contrast**, as the factor its differences are
    /// scaled by: the spread of lightness its visible pixels use (5th to 95th
    /// percentile) set against 60 of Lab's 100, from 1 for a picture that uses
    /// the range to 4 for a night scene that lives in the bottom fifth of it.
    /// A tolerance is then a share of what the picture itself shows: in a
    /// night village the road, the house walls and the sky sit within ΔE 13
    /// of each other and are still plainly different things.
    fn contrast(&self) -> f32 {
        let step = ((self.w * self.h) as f64 / 40_000.0).sqrt().max(1.0) as usize;
        let mut ls: Vec<u8> = (0..self.w * self.h).step_by(step).filter(|i| self.alpha[*i] > 0).map(|i| self.lab[i][0]).collect();
        if ls.len() < 16 {
            return 1.0;
        }
        ls.sort_unstable();
        let spread = f32::from(ls[ls.len() * 95 / 100].saturating_sub(ls[ls.len() * 5 / 100])) / 2.5;
        (60.0 / spread.max(1.0)).clamp(1.0, 4.0)
    }

    /// The difference between pixel `i` and a colour, in ΔE (Lab units),
    /// with alpha judged as in [`alike`]: colour counts as much as the less
    /// visible of the two is seen, and a change of alpha counts in full.
    fn to(&self, i: usize, c: [u8; 3], alpha: u8) -> f32 {
        let p = self.lab[i];
        let dl = (f32::from(p[0]) - f32::from(c[0])) / 2.5;
        let da = f32::from(p[1]) - f32::from(c[1]);
        let db = f32::from(p[2]) - f32::from(c[2]);
        let visible = f32::from(self.alpha[i].min(alpha)) / 255.0;
        ((dl * dl + da * da + db * db).sqrt() * visible * self.scale).max(f32::from(self.alpha[i].abs_diff(alpha)) / 2.55)
    }

    fn between(&self, i: usize, j: usize) -> f32 {
        self.to(i, self.lab[j], self.alpha[j])
    }

    /// Is pixel `i` on an edge: a neighbour differs from it by more than `at`.
    fn edge(&self, i: usize, at: f32) -> bool {
        let (x, y) = (i % self.w, i / self.w);
        (x > 0 && self.between(i, i - 1) > at)
            || (x + 1 < self.w && self.between(i, i + 1) > at)
            || (y > 0 && self.between(i, i - self.w) > at)
            || (y + 1 < self.h && self.between(i, i + self.w) > at)
    }

    /// The 4-neighbours of `i` inside the picture.
    fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let (x, y) = (i % self.w, i / self.w);
        [
            (x > 0).then(|| i - 1),
            (x + 1 < self.w).then(|| i + 1),
            (y > 0).then(|| i - self.w),
            (y + 1 < self.h).then(|| i + self.w),
        ]
        .into_iter()
        .flatten()
    }
}

/// Every pixel the wand takes, starting from `seed`.
///
/// `None` when the seed is outside the picture.
pub fn flood(asset: &ImageAsset, seed: (i64, i64), options: WandOptions) -> Option<Mask> {
    if !options.stop_at_lines && !options.catch_gaps && !options.smooth {
        return flood_plain(asset, seed, options);
    }
    let (w, h) = (asset.width as i64, asset.height as i64);
    if seed.0 < 0 || seed.1 < 0 || seed.0 >= w || seed.1 >= h || w == 0 || h == 0 {
        return None;
    }
    let pic = LabPicture::new(asset);
    let (wu, hu) = (w as usize, h as usize);
    // The tolerance's 0–255 in ΔE: the default 32 is a ΔE of 10, the point
    // at which two colours are plainly different side by side.
    let tolerance = (options.tolerance.clamp(0.0, 1.0) * 255.0 / 3.2) as f32;
    let seed_i = seed.1 as usize * wu + seed.0 as usize;

    // **The colour clicked**: the average of the pixels round the click that
    // are within tolerance of it, so the grain of a painted sky or a JPEG
    // does not decide the answer by which pixel was under the pointer.
    let mut sum = [0f32; 4];
    let mut n = 0f32;
    let mut window = Vec::new();
    for dy in -2i64..=2 {
        for dx in -2i64..=2 {
            let (x, y) = (seed.0 + dx, seed.1 + dy);
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            window.push(y as usize * wu + x as usize);
        }
    }
    for &i in &window {
        if pic.between(i, seed_i) <= tolerance.max(1.0) {
            for k in 0..3 {
                sum[k] += f32::from(pic.lab[i][k]);
            }
            sum[3] += f32::from(pic.alpha[i]);
            n += 1.0;
        }
    }
    let target: [u8; 3] = std::array::from_fn(|k| (sum[k] / n.max(1.0)).round() as u8);
    let target_alpha = (sum[3] / n.max(1.0)).round() as u8;

    // **What counts as an edge here**: several times the grain between
    // neighbouring pixels round the click (a textured ground has more of it
    // than a smooth sky), and never less than a third of the tolerance.
    let edge_at = if options.stop_at_lines {
        let mut steps: Vec<f32> = Vec::new();
        for dy in -4i64..=4 {
            for dx in -4i64..4 {
                let (x, y) = (seed.0 + dx, seed.1 + dy);
                if x < 0 || y < 0 || x + 1 >= w || y >= h {
                    continue;
                }
                let i = y as usize * wu + x as usize;
                steps.push(pic.between(i, i + 1));
            }
        }
        steps.sort_by(f32::total_cmp);
        let grain = steps.get(steps.len() / 2).copied().unwrap_or(0.0);
        (grain * 4.0).max(tolerance / 3.0).max(2.5)
    } else {
        f32::INFINITY
    };

    let mut bits = vec![false; wu * hu];
    if !options.contiguous {
        for (i, b) in bits.iter_mut().enumerate() {
            *b = pic.to(i, target, target_alpha) <= tolerance;
        }
    } else {
        grow(&pic, seed_i, target, target_alpha, tolerance, edge_at, &mut bits);
        if options.catch_gaps {
            catch_gaps(&pic, target, target_alpha, tolerance, edge_at, &mut bits);
        }
    }
    if options.smooth {
        smooth_mask(&pic, target, target_alpha, tolerance, &mut bits);
    }
    let count = bits.iter().filter(|b| **b).count();
    Some(Mask { width: asset.width, height: asset.height, bits, count })
}

/// **The contiguous region**, grown from the click: a pixel is taken when it
/// is within tolerance of the colour clicked — or, a step on from a pixel
/// already taken, when it differs from that one by little and from the colour
/// clicked by no more than three times the tolerance — eight, when the step
/// is very gentle — which is what follows a sky down its gradient to the
/// horizon. A pixel on an edge is taken but grows no further, so
/// a drawn line stops the region however alike the colours either side are.
fn grow(pic: &LabPicture, seed: usize, target: [u8; 3], alpha: u8, tolerance: f32, edge_at: f32, bits: &mut [bool]) {
    if pic.to(seed, target, alpha) > tolerance.max(1.0) * 2.0 {
        return;
    }
    // A smooth sky in 8-bit steps now and then ticks all three channels at
    // once (about ΔE 2), still far below an edge: the floor keeps it gentle.
    let gentle = (edge_at * 0.5).max(2.5);
    let mut queue = std::collections::VecDeque::new();
    bits[seed] = true;
    queue.push_back(seed);
    while let Some(i) = queue.pop_front() {
        if i != seed && pic.edge(i, edge_at) {
            continue;
        }
        for j in pic.neighbours(i) {
            if bits[j] {
                continue;
            }
            let to_target = pic.to(j, target, alpha);
            let step = pic.between(j, i);
            // A sky changes by far less than an edge from one pixel to the
            // next, so a very gentle step is followed much further.
            let reach = if step <= gentle * 0.6 { 8.0 } else if step <= gentle { 3.0 } else { 1.0 };
            if to_target <= tolerance * reach {
                bits[j] = true;
                queue.push_back(j);
            }
        }
    }
}

/// **The same colour close by**: pockets of it that the region nearly
/// reaches — sky between fronds, ground between blades — joined to it. A
/// pocket counts when it is within a short reach of the region (about half a
/// percent of the picture's size) and smaller than a quarter of it, so a
/// second sky on the far side of a house is still a second click.
fn catch_gaps(pic: &LabPicture, target: [u8; 3], alpha: u8, tolerance: f32, edge_at: f32, bits: &mut [bool]) {
    let (w, h) = (pic.w, pic.h);
    let reach = ((w.max(h) as f32) * 0.006).round().max(4.0) as usize;
    let main = bits.iter().filter(|b| **b).count();
    if main == 0 {
        return;
    }
    let near = dilate(bits, w, h, reach);
    let candidate = |i: usize| pic.to(i, target, alpha) <= tolerance && !pic.edge(i, edge_at);
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    for start in 0..w * h {
        if bits[start] || seen[start] || !near[start] || !candidate(start) {
            continue;
        }
        // One pocket, walked whole; taken when it is small beside the region.
        let mut pocket = Vec::new();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            pocket.push(i);
            for j in pic.neighbours(i) {
                if !seen[j] && !bits[j] && candidate(j) {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        if pocket.len() * 4 <= main {
            for i in pocket {
                bits[i] = true;
            }
        }
    }
    // Their rims: the edge pixels round each pocket, within tolerance.
    let grown = dilate(bits, w, h, 1);
    for i in 0..w * h {
        if grown[i] && !bits[i] && pic.to(i, target, alpha) <= tolerance {
            bits[i] = true;
        }
    }
}

/// `bits` grown by `r` pixels in every direction (a square neighbourhood):
/// along every row, then along every column, each line on its own core.
fn dilate(bits: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    use rayon::prelude::*;
    if r == 0 {
        return bits.to_vec();
    }
    // Along one line: a cell is near when a taken one is within r of it.
    fn line(src: &[bool], dst: &mut [bool], r: usize) {
        let len = src.len();
        let mut last: Option<usize> = None;
        let mut next = usize::MAX;
        // The first taken cell at or after k, found by walking ahead once.
        let mut ahead = 0usize;
        for k in 0..len {
            if src[k] {
                last = Some(k);
            }
            if next == usize::MAX || next < k {
                next = usize::MAX;
                ahead = ahead.max(k);
                while ahead < len && ahead <= k + r {
                    if src[ahead] {
                        next = ahead;
                        break;
                    }
                    ahead += 1;
                }
            }
            dst[k] = last.is_some_and(|l| k - l <= r) || (next != usize::MAX && next - k <= r);
        }
    }
    let mut rows = vec![false; w * h];
    rows.par_chunks_mut(w).zip(bits.par_chunks(w)).for_each(|(dst, src)| line(src, dst, r));
    let columns: Vec<Vec<bool>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let src: Vec<bool> = (0..h).map(|y| rows[y * w + x]).collect();
            let mut dst = vec![false; h];
            line(&src, &mut dst, r);
            dst
        })
        .collect();
    let mut out = vec![false; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, cell) in row.iter_mut().enumerate() {
            *cell = columns[x][y];
        }
    });
    out
}

/// `bits` shrunk by `r` pixels: the complement of the complement grown.
fn erode(bits: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let inverse: Vec<bool> = bits.iter().map(|b| !b).collect();
    dilate(&inverse, w, h, r).into_iter().map(|b| !b).collect()
}

/// **Nothing sticking out, nothing left behind**: a closing then an opening
/// of one pixel (two on a very large picture) takes off the spikes and fills
/// the nicks along the edge; holes inside that are specks — tiny, or small and
/// close to the colour clicked — are filled; and loose crumbs away from the
/// region are dropped.
fn smooth_mask(pic: &LabPicture, target: [u8; 3], alpha: u8, tolerance: f32, bits: &mut Vec<bool>) {
    let (w, h) = (pic.w, pic.h);
    let total = bits.iter().filter(|b| **b).count();
    if total == 0 {
        return;
    }
    let r = (w.max(h) / 2400).max(1);
    let closed = erode(&dilate(bits, w, h, r), w, h, r);
    let opened = dilate(&erode(&closed, w, h, r), w, h, r);
    // An opening can wipe out a region narrower than itself; keep the closed
    // one then rather than lose what was clicked.
    *bits = if opened.iter().filter(|b| **b).count() * 2 >= total { opened } else { closed };

    let speck = ((w * h) as f64 * 0.000_02).max(16.0) as usize;
    let small = ((w * h) as f64 * 0.000_5).max(64.0) as usize;
    let total = bits.iter().filter(|b| **b).count();
    for value in [false, true] {
        let mut seen = vec![false; w * h];
        let mut stack = Vec::new();
        for start in 0..w * h {
            if bits[start] != value || seen[start] {
                continue;
            }
            let mut part = Vec::new();
            let mut border = false;
            seen[start] = true;
            stack.push(start);
            while let Some(i) = stack.pop() {
                part.push(i);
                let (x, y) = (i % w, i / w);
                border |= x == 0 || y == 0 || x + 1 == w || y + 1 == h;
                for j in pic.neighbours(i) {
                    if !seen[j] && bits[j] == value {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            if !value {
                // A hole (or the outside): filled when it is a speck, or small
                // and mostly near the colour clicked.
                if border || part.len() > small {
                    continue;
                }
                let like = part.len() <= speck || {
                    let close = part.iter().filter(|i| pic.to(**i, target, alpha) <= tolerance * 2.0).count();
                    close * 2 >= part.len()
                };
                if like {
                    part.into_iter().for_each(|i| bits[i] = true);
                }
            } else if part.len() <= speck && part.len() * 4 < total {
                // A crumb on its own: gone.
                part.into_iter().for_each(|i| bits[i] = false);
            }
        }
    }
}

/// The plain wand's flood: the colour clicked and the tolerance, nothing else.
fn flood_plain(asset: &ImageAsset, seed: (i64, i64), options: WandOptions) -> Option<Mask> {
    let (w, h) = (asset.width as i64, asset.height as i64);
    if seed.0 < 0 || seed.1 < 0 || seed.0 >= w || seed.1 >= h || w == 0 || h == 0 {
        return None;
    }
    let target = asset.pixel(seed.0, seed.1);
    let tolerance_255 = options.tolerance.clamp(0.0, 1.0) * 255.0;

    let mut bits = vec![false; (w * h) as usize];
    let mut count = 0usize;

    if !options.contiguous {
        // Everything that matches, wherever it is. One sweep, no search.
        for y in 0..h {
            for x in 0..w {
                if alike(asset.pixel(x, y), target, tolerance_255) {
                    bits[(y * w + x) as usize] = true;
                    count += 1;
                }
            }
        }
        return Some(Mask {
            width: asset.width,
            height: asset.height,
            bits,
            count,
        });
    }

    // Scanline flood fill: each stack entry is a whole run, not a pixel, so a
    // sky is a few thousand pushes rather than a few million. The recursive
    // four-neighbour version overflows the stack on any real photograph.
    let mut stack = vec![(seed.0, seed.1)];
    while let Some((sx, sy)) = stack.pop() {
        if bits[(sy * w + sx) as usize] || !alike(asset.pixel(sx, sy), target, tolerance_255) {
            continue;
        }
        // Walk left and right to the ends of this run.
        let mut left = sx;
        while left > 0
            && !bits[(sy * w + left - 1) as usize]
            && alike(asset.pixel(left - 1, sy), target, tolerance_255)
        {
            left -= 1;
        }
        let mut right = sx;
        while right + 1 < w
            && !bits[(sy * w + right + 1) as usize]
            && alike(asset.pixel(right + 1, sy), target, tolerance_255)
        {
            right += 1;
        }
        for x in left..=right {
            bits[(sy * w + x) as usize] = true;
            count += 1;
        }
        // Seed the rows above and below, one push per unbroken run rather than
        // one per pixel.
        for ny in [sy - 1, sy + 1] {
            if ny < 0 || ny >= h {
                continue;
            }
            let mut x = left;
            while x <= right {
                if !bits[(ny * w + x) as usize] && alike(asset.pixel(x, ny), target, tolerance_255)
                {
                    stack.push((x, ny));
                    // Skip the rest of this run: the push above will take it.
                    while x <= right
                        && alike(asset.pixel(x, ny), target, tolerance_255)
                        && !bits[(ny * w + x) as usize]
                    {
                        x += 1;
                    }
                } else {
                    x += 1;
                }
            }
        }
    }

    Some(Mask {
        width: asset.width,
        height: asset.height,
        bits,
        count,
    })
}

/// The outline of a mask, in **pixel** coordinates.
///
/// Outer boundaries are wound one way and holes the other, so filling the
/// result non-zero leaves the holes open.
pub fn trace(mask: &Mask, simplify: f64) -> BezPath {
    trace_loops(mask, simplify, false)
}

/// [`trace`], with every loop rounded after it is simplified (one pass of
/// Chaikin's corner cutting), so the cut edge runs smooth instead of in pixel
/// steps.
pub fn trace_smooth(mask: &Mask, simplify: f64) -> BezPath {
    trace_loops(mask, simplify, true)
}

fn trace_loops(mask: &Mask, simplify: f64, round: bool) -> BezPath {
    // Every boundary edge of every taken pixel, directed so that the inside is
    // always on the same hand. Four cases, one per side.
    let mut out: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
    let mut edges = 0usize;
    let mut push = |a: (i32, i32), b: (i32, i32)| {
        out.entry(a).or_default().push(b);
    };
    for y in 0..mask.height as i64 {
        for x in 0..mask.width as i64 {
            if !mask.get(x, y) {
                continue;
            }
            let (xi, yi) = (x as i32, y as i32);
            if !mask.get(x, y - 1) {
                push((xi, yi), (xi + 1, yi));
                edges += 1;
            }
            if !mask.get(x + 1, y) {
                push((xi + 1, yi), (xi + 1, yi + 1));
                edges += 1;
            }
            if !mask.get(x, y + 1) {
                push((xi + 1, yi + 1), (xi, yi + 1));
                edges += 1;
            }
            if !mask.get(x - 1, y) {
                push((xi, yi + 1), (xi, yi));
                edges += 1;
            }
        }
    }

    // Chain the edges into closed loops. Every lattice point has as many edges
    // leaving it as arriving, so following edges until none is left always
    // closes — there is no dead end to guard against.
    let mut loops: Vec<Vec<(i32, i32)>> = Vec::new();
    let starts: Vec<(i32, i32)> = out.keys().copied().collect();
    let mut walked = 0usize;
    for start in starts {
        while out.get(&start).is_some_and(|v| !v.is_empty()) {
            let mut path = vec![start];
            let mut at = start;
            while let Some(next) = out.get_mut(&at).and_then(|v| v.pop()) {
                walked += 1;
                if next == start {
                    break;
                }
                path.push(next);
                at = next;
                // A malformed mask cannot happen, but a runaway loop here
                // would hang the editor, so it is bounded by construction.
                if walked > edges {
                    break;
                }
            }
            if path.len() >= 4 {
                loops.push(path);
            }
        }
    }

    build_path(loops, simplify, round)
}

/// Turn traced loops into one path, simplified to fit the point budget.
fn build_path(loops: Vec<Vec<(i32, i32)>>, simplify: f64, round: bool) -> BezPath {
    let mut epsilon = simplify.max(0.0);
    loop {
        let mut path = BezPath::new();
        let mut points = 0usize;
        for lp in &loops {
            let pts: Vec<Point> = lp
                .iter()
                .map(|&(x, y)| Point::new(f64::from(x), f64::from(y)))
                .collect();
            let pts = drop_collinear(&pts);
            let pts = if epsilon > 0.0 {
                simplify_closed(&pts, epsilon)
            } else {
                pts
            };
            let pts = if round && pts.len() >= 4 { chaikin(&pts) } else { pts };
            if pts.len() < 3 || signed_area(&pts).abs() < MIN_LOOP_AREA_PX {
                continue;
            }
            points += pts.len();
            path.move_to(pts[0]);
            for p in &pts[1..] {
                path.line_to(*p);
            }
            path.close_path();
        }
        if points <= MAX_REGION_POINTS || epsilon > 1e4 {
            return path;
        }
        epsilon = if epsilon <= 0.0 { 1.0 } else { epsilon * 2.0 };
    }
}

/// One pass of Chaikin's corner cutting on a closed loop, **bounded**: every
/// corner bevelled by a quarter of its edges but never more than one and a
/// half pixels, so the pixel steps along an edge read as a curve while a real
/// corner — the picture's own, or a roof's — stays where it is. (Unbounded,
/// a quarter of a 1,920-pixel edge took 480 pixels off each corner of a sky.)
fn chaikin(pts: &[Point]) -> Vec<Point> {
    const MAX_CUT: f64 = 1.5;
    let mut out = Vec::with_capacity(pts.len() * 2);
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        let len = (b - a).hypot();
        if len <= f64::EPSILON {
            continue;
        }
        let t = (MAX_CUT / len).min(0.25);
        out.push(a.lerp(b, t));
        out.push(a.lerp(b, 1.0 - t));
    }
    out
}

/// Twice the signed area, by the shoelace formula.
fn signed_area(pts: &[Point]) -> f64 {
    let mut sum = 0.0;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        sum += a.x * b.y - b.x * a.y;
    }
    sum / 2.0
}

/// Drop points that lie on the segment between their neighbours.
///
/// Exact, and it does most of the work: a traced boundary is unit steps, so
/// every straight run — which is most of any real edge — collapses to one
/// segment with no loss at all.
fn drop_collinear(pts: &[Point]) -> Vec<Point> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut kept: Vec<Point> = Vec::with_capacity(pts.len());
    for i in 0..pts.len() {
        let prev = pts[(i + pts.len() - 1) % pts.len()];
        let here = pts[i];
        let next = pts[(i + 1) % pts.len()];
        let a = here - prev;
        let b = next - here;
        if (a.x * b.y - a.y * b.x).abs() > 1e-12 {
            kept.push(here);
        }
    }
    if kept.len() < 3 { pts.to_vec() } else { kept }
}

/// Douglas–Peucker on a closed loop.
///
/// A closed loop has no ends to anchor the recursion, so the two points
/// furthest apart are used as the ends of two open runs. That is stable — it
/// depends on the shape, not on where the trace happened to start.
fn simplify_closed(pts: &[Point], epsilon: f64) -> Vec<Point> {
    if pts.len() < 4 {
        return pts.to_vec();
    }
    // The leftmost and rightmost points. Cheaper than an all-pairs search for
    // the true diameter, and all that is wanted is a pair far enough apart to
    // anchor the two halves — and one chosen by the *shape*, so the result does
    // not depend on where the trace happened to begin.
    let (mut lo, mut hi) = (0usize, 0usize);
    for (i, p) in pts.iter().enumerate() {
        if (p.x, p.y) < (pts[lo].x, pts[lo].y) {
            lo = i;
        }
        if (p.x, p.y) > (pts[hi].x, pts[hi].y) {
            hi = i;
        }
    }
    if lo == hi {
        hi = (lo + pts.len() / 2) % pts.len();
    }
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }

    // Two open runs between the same two anchors, going opposite ways round.
    let forward: Vec<Point> = pts[lo..=hi].to_vec();
    let mut back: Vec<Point> = pts[hi..].to_vec();
    back.extend_from_slice(&pts[..=lo]);

    let mut out = douglas_peucker(&forward, epsilon);
    let tail = douglas_peucker(&back, epsilon);
    // `out` already ends at the far anchor and begins at the near one, and the
    // return run carries both again — so only its interior is added.
    let interior = tail.len().saturating_sub(2);
    out.extend(tail.into_iter().skip(1).take(interior));
    out
}

fn douglas_peucker(pts: &[Point], epsilon: f64) -> Vec<Point> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (first, last) = (pts[0], pts[pts.len() - 1]);
    let mut worst = (0usize, 0.0);
    for (i, p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
        let d = distance_to_segment(*p, first, last);
        if d > worst.1 {
            worst = (i, d);
        }
    }
    if worst.1 <= epsilon {
        return vec![first, last];
    }
    let mut left = douglas_peucker(&pts[..=worst.0], epsilon);
    let right = douglas_peucker(&pts[worst.0..], epsilon);
    left.pop();
    left.extend(right);
    left
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 <= f64::MIN_POSITIVE {
        return (p - a).hypot();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).hypot()
}

/// The whole wand: a point in the shape's own space to a path in that same
/// space, ready to be cut out of it.
///
/// `None` when the click misses the picture or the fill's transform has
/// collapsed, and when the region traced to nothing.
pub fn region_at(fill: &ImageFill, local: Point, options: WandOptions) -> Option<BezPath> {
    let pixel = fill.to_pixel(local)?;
    let seed = (pixel.x.floor() as i64, pixel.y.floor() as i64);
    let mask = flood(&fill.asset, seed, options)?;
    if mask.is_empty() {
        return None;
    }
    let traced = if options.smooth { trace_smooth(&mask, options.simplify) } else { trace(&mask, options.simplify) };
    if traced.is_empty() {
        return None;
    }
    // Pixel space back to the object's own space: the unit square scaled up to
    // the picture's pixel dimensions is exactly what `ImageFill::from_pixel`
    // undoes, written as one matrix so a whole path goes through at once.
    let to_local = fill.transform
        * Affine::scale_non_uniform(
            1.0 / f64::from(fill.asset.width).max(1.0),
            1.0 / f64::from(fill.asset.height).max(1.0),
        );
    Some(to_local * traced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Rect, Shape as _};
    use std::sync::Arc;

    /// A picture with a red disc on a blue ground.
    fn disc(size: u32, radius: f64) -> Arc<ImageAsset> {
        let c = f64::from(size) / 2.0;
        let mut pixels = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let d =
                    ((f64::from(x) + 0.5 - c).powi(2) + (f64::from(y) + 0.5 - c).powi(2)).sqrt();
                if d <= radius {
                    pixels.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    pixels.extend_from_slice(&[0, 0, 255, 255]);
                }
            }
        }
        Arc::new(ImageAsset::from_pixels(
            crate::ImageId(1),
            "Disc",
            size,
            size,
            Arc::new(pixels),
        ))
    }

    #[test]
    fn a_click_inside_the_disc_takes_the_disc_and_nothing_else() {
        let asset = disc(64, 20.0);
        let mask = flood(&asset, (32, 32), WandOptions::default()).expect("seed inside");
        let area = std::f64::consts::PI * 20.0 * 20.0;
        let taken = mask.count() as f64;
        assert!(
            (taken - area).abs() / area < 0.05,
            "took {taken} pixels, the disc is about {area}"
        );
        // And the corner, which is ground, was not taken.
        assert!(!mask.get(0, 0));
    }

    /// **The plain wand: tolerance is the whole tool.** Zero takes only the
    /// exact colour.
    #[test]
    fn tolerance_decides_how_much_is_taken() {
        // A ramp: every column one step brighter than the last.
        let mut pixels = Vec::new();
        for _ in 0..8u32 {
            for x in 0..64u32 {
                let v = x as u8 * 4;
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let asset = Arc::new(ImageAsset::from_pixels(
            crate::ImageId(2),
            "Ramp",
            64,
            8,
            Arc::new(pixels),
        ));

        let exact = flood(&asset, (32, 4), WandOptions::plain(0.0, true)).unwrap();
        assert_eq!(exact.count(), 8, "only its own column");

        // Sixteen levels of 4 either way: nine columns.
        let loose = flood(&asset, (32, 4), WandOptions::plain(16.0 / 255.0, true)).unwrap();
        assert_eq!(loose.count(), 9 * 8);
    }

    /// Contiguous is what separates "this patch of sky" from "all the sky".
    #[test]
    fn contiguous_stops_at_the_wall_and_global_does_not() {
        // Two white squares divided by a black wall.
        let (w, h) = (32u32, 8u32);
        let mut pixels = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                let v = if (14..18).contains(&x) { 0u8 } else { 255 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let asset = Arc::new(ImageAsset::from_pixels(
            crate::ImageId(3),
            "Wall",
            w,
            h,
            Arc::new(pixels),
        ));

        let near = flood(&asset, (2, 4), WandOptions::default()).unwrap();
        assert_eq!(near.count(), 14 * 8, "stopped at the wall");
        assert!(!near.get(20, 4), "and did not jump it");

        let all = flood(
            &asset,
            (2, 4),
            WandOptions {
                contiguous: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.count(), (14 + 14) * 8, "both sides");
        assert!(all.get(20, 4));
    }

    /// **The traced path encloses what was taken and excludes what was not.**
    ///
    /// The claim that matters: an outline that merely exists proves nothing,
    /// but one that contains the disc's centre and not the picture's corner is
    /// round the right thing.
    #[test]
    fn the_traced_path_encloses_the_region() {
        let asset = disc(64, 20.0);
        let mask = flood(&asset, (32, 32), WandOptions::default()).unwrap();
        let path = trace(&mask, 1.0);
        assert!(!path.is_empty());

        assert!(
            path.contains(Point::new(32.0, 32.0)),
            "the centre of the disc is inside its own outline"
        );
        assert!(
            !path.contains(Point::new(1.0, 1.0)),
            "the corner is ground, and outside"
        );
        // Its area is the disc's, not the whole picture's.
        let area = path.area().abs();
        let want = std::f64::consts::PI * 20.0 * 20.0;
        assert!(
            (area - want).abs() / want < 0.1,
            "outline encloses {area:.0}, the disc is {want:.0}"
        );
    }

    /// **A hole stays a hole.** Winding, not luck.
    #[test]
    fn a_ring_traces_with_its_hole_open() {
        let size = 64u32;
        let c = f64::from(size) / 2.0;
        let mut pixels = Vec::new();
        for y in 0..size {
            for x in 0..size {
                let d =
                    ((f64::from(x) + 0.5 - c).powi(2) + (f64::from(y) + 0.5 - c).powi(2)).sqrt();
                // A ring: taken between 10 and 24.
                let inside = (10.0..24.0).contains(&d);
                if inside {
                    pixels.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    pixels.extend_from_slice(&[0, 0, 255, 255]);
                }
            }
        }
        let asset = Arc::new(ImageAsset::from_pixels(
            crate::ImageId(4),
            "Ring",
            size,
            size,
            Arc::new(pixels),
        ));

        let mask = flood(&asset, (32, 32 - 17), WandOptions::default()).unwrap();
        let path = trace(&mask, 0.5);

        assert!(
            path.contains(Point::new(32.0, 32.0 - 17.0)),
            "the ring itself is inside"
        );
        assert!(
            !path.contains(Point::new(32.0, 32.0)),
            "the middle of the ring is a hole, not filled"
        );
    }

    /// Simplification keeps the shape and loses the points.
    #[test]
    fn simplifying_keeps_the_area_and_drops_the_points() {
        let asset = disc(256, 100.0);
        let mask = flood(&asset, (128, 128), WandOptions::default()).unwrap();
        let exact = trace(&mask, 0.0);
        let eased = trace(&mask, 1.0);

        let n_exact = exact.elements().len();
        let n_eased = eased.elements().len();
        assert!(
            n_eased * 4 < n_exact,
            "simplifying saved little: {n_exact} to {n_eased}"
        );
        let (a, b) = (exact.area().abs(), eased.area().abs());
        assert!(
            (a - b).abs() / a < 0.02,
            "simplifying changed the area: {a:.0} to {b:.0}"
        );
    }

    /// The path comes back in the shape's own space, not in pixels.
    #[test]
    fn the_region_lands_where_the_picture_is() {
        let asset = disc(64, 20.0);
        let placed = Rect::new(1000.0, 500.0, 1128.0, 628.0); // 2x, moved
        let fill = ImageFill::new(asset, placed);

        let centre = placed.center();
        let region = region_at(&fill, centre, WandOptions::default()).expect("a region");

        assert!(region.contains(centre), "the disc is under the click");
        assert!(
            !region.contains(Point::new(1002.0, 502.0)),
            "the corner of the placement is ground"
        );
        // Twice the scale is four times the area.
        let want = std::f64::consts::PI * 40.0 * 40.0;
        let area = region.area().abs();
        assert!(
            (area - want).abs() / want < 0.1,
            "region covers {area:.0}, expected about {want:.0}"
        );
    }

    /// A wand on a picture it misses catches nothing, rather than panicking.
    #[test]
    fn a_click_off_the_picture_takes_nothing() {
        let fill = ImageFill::new(disc(64, 20.0), Rect::new(0.0, 0.0, 64.0, 64.0));
        assert!(region_at(&fill, Point::new(-50.0, -50.0), WandOptions::default()).is_none());
    }

    /// Growing a mask matches the plain definition — taken when a taken pixel
    /// is within r along both axes — on scattered masks of awkward sizes.
    #[test]
    fn growing_a_mask_matches_the_definition() {
        let mut seed = 12345u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for (w, h, r) in [(17usize, 9usize, 1usize), (31, 23, 2), (8, 40, 3), (50, 1, 4)] {
            let bits: Vec<bool> = (0..w * h).map(|_| next() % 23 == 0).collect();
            let grown = dilate(&bits, w, h, r);
            for y in 0..h {
                for x in 0..w {
                    let want = (y.saturating_sub(r)..=(y + r).min(h - 1))
                        .any(|yy| (x.saturating_sub(r)..=(x + r).min(w - 1)).any(|xx| bits[yy * w + xx]));
                    assert_eq!(grown[y * w + x], want, "{w}x{h} r{r} at {x},{y}");
                }
            }
        }
    }

    /// **Rounding never cuts a real corner**: a big rectangle keeps its
    /// corners within a pixel and a half, and its area within a hair.
    #[test]
    fn rounding_keeps_big_corners() {
        let square = vec![Point::new(0.0, 0.0), Point::new(1000.0, 0.0), Point::new(1000.0, 600.0), Point::new(0.0, 600.0)];
        let round = chaikin(&square);
        let near_corner = round.iter().map(|p| (p.x.powi(2) + p.y.powi(2)).sqrt()).fold(f64::MAX, f64::min);
        assert!(near_corner < 2.2, "the corner is still there: closest point {near_corner}");
        let lost = signed_area(&square).abs() - signed_area(&round).abs();
        assert!(lost < 10.0, "lost {lost} of the area");
    }

    /// Paint a picture from a function of the pixel.
    fn picture(w: u32, h: u32, paint: impl Fn(u32, u32) -> [u8; 4]) -> Arc<ImageAsset> {
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&paint(x, y));
            }
        }
        Arc::new(ImageAsset::from_pixels(crate::ImageId(7), "Painted", w, h, Arc::new(pixels)))
    }

    /// **A night scene stops at its outlines.** A dark road, a black outline
    /// two pixels wide (with a grey step either side, as anti-aliasing draws
    /// it) and a dark-red door beyond: all within 32 levels of each other,
    /// so the plain wand takes the lot; the wand stops at the line.
    #[test]
    fn a_dark_road_stops_at_its_outline() {
        let asset = picture(120, 60, |x, _| match x {
            0..=56 => [24, 17, 7, 255],
            57 => [12, 8, 4, 255],
            58..=59 => [0, 0, 0, 255],
            60 => [20, 10, 8, 255],
            _ => [45, 18, 14, 255],
        });
        let plain = flood(&asset, (10, 30), WandOptions::plain(32.0 / 255.0, true)).unwrap();
        assert!(plain.get(100, 30), "the plain wand runs over the line into the door");
        let wand = flood(&asset, (10, 30), WandOptions::default()).unwrap();
        assert!(wand.get(55, 30), "the road is taken up to the line");
        assert!(!wand.get(59, 30) && !wand.get(100, 30), "the line and the door are not");
    }

    /// **A sky follows its gradient to the horizon**, and **pockets of it**
    /// between the fronds of a tree are taken with it, while the tree is not.
    #[test]
    fn a_sky_is_taken_down_its_gradient_and_between_the_leaves() {
        let (w, h) = (200u32, 120u32);
        let asset = picture(w, h, |x, y| {
            // A dark tree from x 60 to 140 below y 30, with small holes of sky
            // through its crown just under its edge (every 12 px, 4 across).
            let in_tree = (60..140).contains(&x) && y >= 30;
            let hole = in_tree && (32..36).contains(&y) && (x % 12) < 4;
            if in_tree && !hole {
                [10, 30, 12, 255]
            } else {
                // Sky: lighter towards the bottom, one level every 4 rows.
                let v = 60 + (y / 4) as u8;
                [v, v + 10, v + 40, 255]
            }
        });
        let mask = flood(&asset, (10, 5), WandOptions::default()).unwrap();
        assert!(mask.get(10, 115), "the sky right down at the horizon");
        assert!(mask.get(73, 33), "a hole of sky through the crown");
        assert!(!mask.get(100, 80), "not the tree");
    }

    /// **Nothing left behind**: a speck of grain inside a flat region is
    /// filled, and a one-pixel spike of the region off its edge is shaved.
    #[test]
    fn specks_are_filled_and_spikes_shaved() {
        let asset = picture(80, 80, |x, y| {
            if (40..42).contains(&x) && (40..42).contains(&y) {
                [255, 255, 255, 255] // a white speck in the blue
            } else if x >= 60 && !(y == 30 && x < 70) {
                [200, 40, 30, 255] // red, with a one-pixel blue spike into it
            } else {
                [20, 40, 200, 255]
            }
        });
        let mask = flood(&asset, (10, 10), WandOptions::default()).unwrap();
        assert!(mask.get(40, 40), "the speck is filled");
        assert!(!mask.get(66, 30), "the spike into the red is shaved off");
        assert!(!mask.get(70, 10), "the red is not taken");
    }

    /// Big enough to be a real photograph, and it must not take all day.
    ///
    /// The whole point of the tool is that it answers while the pointer is
    /// still down. Eight megapixels is a phone photograph.
    #[test]
    fn a_photograph_sized_picture_is_traced_promptly() {
        let asset = disc(2048, 700.0);
        let start = std::time::Instant::now();
        let region = region_at(
            &ImageFill::new(Arc::clone(&asset), Rect::new(0.0, 0.0, 2048.0, 2048.0)),
            Point::new(1024.0, 1024.0),
            WandOptions::default(),
        )
        .expect("a region");
        let took = start.elapsed();
        assert!(!region.is_empty());
        assert!(
            took.as_millis() < 2000,
            "the wand took {took:?} on a 4-megapixel picture"
        );
        assert!(
            region.elements().len() < MAX_REGION_POINTS,
            "the region came back with {} points",
            region.elements().len()
        );
    }
}
