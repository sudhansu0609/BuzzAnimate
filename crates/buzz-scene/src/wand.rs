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
}

impl Default for WandOptions {
    fn default() -> Self {
        Self {
            tolerance: 32.0 / 255.0,
            contiguous: true,
            simplify: 1.0,
        }
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

/// Every pixel the wand takes, starting from `seed`.
///
/// `None` when the seed is outside the picture.
pub fn flood(asset: &ImageAsset, seed: (i64, i64), options: WandOptions) -> Option<Mask> {
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

    build_path(loops, simplify)
}

/// Turn traced loops into one path, simplified to fit the point budget.
fn build_path(loops: Vec<Vec<(i32, i32)>>, simplify: f64) -> BezPath {
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
    let traced = trace(&mask, options.simplify);
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
        Arc::new(ImageAsset {
            id: crate::ImageId(1),
            name: "Disc".into(),
            source: Arc::new(Vec::new()),
            format: "png".into(),
            width: size,
            height: size,
            pixels: Arc::new(pixels),
            generation: 0,
        })
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

    /// **Tolerance is the whole tool.** Zero takes only the exact colour.
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
        let asset = Arc::new(ImageAsset {
            id: crate::ImageId(2),
            name: "Ramp".into(),
            source: Arc::new(Vec::new()),
            format: "png".into(),
            width: 64,
            height: 8,
            pixels: Arc::new(pixels),
            generation: 0,
        });

        let exact = flood(
            &asset,
            (32, 4),
            WandOptions {
                tolerance: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(exact.count(), 8, "only its own column");

        let loose = flood(
            &asset,
            (32, 4),
            WandOptions {
                // Sixteen levels of 4 either way: nine columns.
                tolerance: 16.0 / 255.0,
                ..Default::default()
            },
        )
        .unwrap();
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
        let asset = Arc::new(ImageAsset {
            id: crate::ImageId(3),
            name: "Wall".into(),
            source: Arc::new(Vec::new()),
            format: "png".into(),
            width: w,
            height: h,
            pixels: Arc::new(pixels),
            generation: 0,
        });

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
        let asset = Arc::new(ImageAsset {
            id: crate::ImageId(4),
            name: "Ring".into(),
            source: Arc::new(Vec::new()),
            format: "png".into(),
            width: size,
            height: size,
            pixels: Arc::new(pixels),
            generation: 0,
        });

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
