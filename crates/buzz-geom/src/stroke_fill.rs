//! **A brush stroke that has lost its spine, found again from its outline.**
//!
//! A brush stroke keeps the samples it was drawn from ([`crate::BrushSpine`])
//! only while its outline is still the one they build. Merge paint fuses a
//! stroke with what it touches, an eraser bites it, and a film drawn before
//! strokes kept their spines has none at all. What is left is a filled
//! outline — and Smooth, Straighten or Curvier on an outline work on its two
//! edges separately, so they round the tips off, pinch the stroke where the
//! edges come close, and on a stroke whose outline is already fine-grained
//! do nothing a second press can see.
//!
//! A long thin fill is still a stroke, though, and its middle can be found
//! again: the two tips are where the outline turns right round, the two
//! edges run between them, and halfway across is the line the brush
//! travelled. Each edge point is kept as an offset from that middle — so far
//! along it, so far across — so the middle can be smoothed, straightened or
//! bent and both edges carried with it, at the width they were drawn.
//!
//! Anything that is not a single long thin outline — a blob, a ring with a
//! hole, strokes fused into a knot — is refused, and is left to the outline
//! operations in [`crate::edit`].

use kurbo::{BezPath, PathEl, Point, Shape as _, Vec2};

use crate::brush::{
    bend_from_chord, gaussian_along, gaussian_at, reshape_scale, smooth_sigma, straighten_points,
    straighten_step,
};

/// A filled stroke's middle and the edges either side of it. See the module.
#[derive(Debug, Clone)]
pub struct FillSpine {
    /// Down the middle of the stroke, from one tip to the other.
    centre: Vec<Point>,
    /// Each point of the one edge as (along, across) the middle where it is
    /// paired, so it can be carried when the middle moves.
    left: Vec<Vec2>,
    /// The other edge, likewise.
    right: Vec<Vec2>,
    /// How far apart the outline was sampled.
    step: f64,
    /// The stroke's mean width.
    width: f64,
}

impl FillSpine {
    /// The middle of `path`, if it is one long thin closed outline.
    pub fn from_outline(path: &BezPath) -> Option<Self> {
        let ring = single_ring(path)?;
        let (perimeter, area) = (ring_length(&ring), ring_area(&ring).abs());
        // A thin stroke's area is its mean width times its length, and its
        // perimeter twice its length.
        let width = 2.0 * area / perimeter;
        if !(width > 0.0) || perimeter < 8.0 * width {
            return None;
        }
        let step = (width / 4.0).min(perimeter / 40.0).max(perimeter / 4000.0);
        let ring = resample_ring(&ring, step);
        let n = ring.len();
        if n < 16 {
            return None;
        }

        // **The tips** are where the outline turns right round: measured
        // over a stroke's width either side, a tip turns through half a
        // circle and a bend in the stroke through much less.
        let k = ((width / step).round() as usize).clamp(1, n / 8);
        let turning: Vec<f64> = (0..n)
            .map(|i| {
                let a = ring[i] - ring[(i + n - k) % n];
                let b = ring[(i + k) % n] - ring[i];
                a.cross(b).atan2(a.dot(b)).abs()
            })
            .collect();
        let tip_a = (0..n).max_by(|&x, &y| turning[x].total_cmp(&turning[y]))?;
        let apart = |i: usize| {
            let d = (i + n - tip_a) % n;
            d.min(n - d)
        };
        let tip_b = (0..n)
            .filter(|&i| apart(i) >= n / 4)
            .max_by(|&x, &y| turning[x].total_cmp(&turning[y]))?;
        if turning[tip_a] < 1.3 || turning[tip_b] < 1.3 {
            return None;
        }

        // **The edges**, both run from the one tip to the other.
        let forward: Vec<Point> = (0..=(tip_b + n - tip_a) % n).map(|d| ring[(tip_a + d) % n]).collect();
        let backward: Vec<Point> = (0..=(tip_a + n - tip_b) % n).map(|d| ring[(tip_a + n - d) % n]).collect();
        let (la, lb) = (polyline_length(&forward), polyline_length(&backward));
        if la.min(lb) < la.max(lb) * 0.5 {
            return None;
        }
        let m = ((la.max(lb) / step).round() as usize).max(8) + 1;
        let left = resample_open(&forward, m);
        let right = resample_open(&backward, m);
        let centre: Vec<Point> = left.iter().zip(&right).map(|(a, b)| a.midpoint(*b)).collect();

        // **Still a stroke?** Long against its widest, and the two edges on
        // either side of the middle all along it — a blob, or tips picked
        // wrongly, fails one or the other.
        let widest = left.iter().zip(&right).map(|(a, b)| (*a - *b).hypot()).fold(0.0, f64::max);
        if polyline_length(&centre) < 3.0 * widest {
            return None;
        }
        let frames = frames_along(&gaussian_along(&centre, width * 0.75));
        let offset = |edge: &[Point]| -> Vec<Vec2> {
            edge.iter()
                .zip(&centre)
                .zip(&frames)
                .map(|((p, c), t)| {
                    let d = *p - *c;
                    Vec2::new(d.dot(*t), d.dot(t.turn_90()))
                })
                .collect()
        };
        let (left, right) = (offset(&left), offset(&right));
        let inner = m / 10..m - m / 10;
        let apart_sides = inner.clone().filter(|&i| left[i].y * right[i].y < 0.0).count();
        if (apart_sides as f64) < inner.len() as f64 * 0.9 {
            return None;
        }

        Some(Self {
            centre,
            left,
            right,
            step,
            width,
        })
    }

    /// Down the middle of the stroke, tip to tip.
    pub fn centre(&self) -> &[Point] {
        &self.centre
    }

    /// **Smoother**, by `strength` (`0`–`1`): the middle evened out and the
    /// width along it with it — see [`crate::BrushSpine::smoothed`]. Every
    /// press goes further.
    ///
    /// The width is evened out over no more than the stroke's own width —
    /// that is the size of a lump in an edge — and then given back its
    /// thickness overall: the tips hold the width at nothing, and averaging
    /// towards them over the stroke's length would thin the whole stroke a
    /// little more with every press.
    pub fn smoother(&self, strength: f64) -> BezPath {
        let sigma = smooth_sigma(reshape_scale(self.width, &self.centre), strength);
        let centre = gaussian_along(&self.centre, sigma);
        let at = crate::brush::arc_lengths(&self.centre);
        let even = |edge: &[Vec2]| -> Vec<Vec2> {
            let mut out = gaussian_at(edge, &at, sigma.min(self.width));
            let (was, now): (f64, f64) = edge
                .iter()
                .zip(&out)
                .fold((0.0, 0.0), |(a, b), (e, o)| (a + e.y.abs(), b + o.y.abs()));
            if now > 1e-12 {
                for o in &mut out {
                    o.y *= was / now;
                }
            }
            out
        };
        self.outline(&centre, &even(&self.left), &even(&self.right))
    }

    /// **Straighter**, by `strength` (`0`–`1`): the middle's wobble pulled part
    /// of the way onto straight runs, the width kept — see
    /// [`crate::BrushSpine::straightened`]. Every press goes further.
    pub fn straighter(&self, strength: f64) -> BezPath {
        let (size, pull) = straighten_step(reshape_scale(self.width, &self.centre), strength);
        self.outline(&straighten_points(&self.centre, size, pull), &self.left, &self.right)
    }

    /// **Curvier**: the middle bowed `amount` further from the line between
    /// the tips — see [`crate::BrushSpine::curvier`]. `None` for a stroke
    /// that comes back on itself.
    pub fn curvier(&self, amount: f64) -> Option<BezPath> {
        let calm = gaussian_along(&self.centre, reshape_scale(self.width, &self.centre) * 0.5);
        let bent = bend_from_chord(&calm, amount.max(0.0))?;
        Some(self.outline(&bent, &self.left, &self.right))
    }

    /// The outline the edges make about `centre`: each edge point carried at
    /// its own offset along and across, the tips where the middle ends.
    pub fn outline_along(&self, centre: &[Point]) -> BezPath {
        self.outline(centre, &self.left, &self.right)
    }

    fn outline(&self, centre: &[Point], left: &[Vec2], right: &[Vec2]) -> BezPath {
        // The direction of travel taken over a stroke's width, the way the
        // offsets were measured: at a corner Straighten has made, the edges
        // swing round it instead of folding over themselves.
        let frames = frames_along(&gaussian_along(centre, self.width * 0.75));
        let place = |offsets: &[Vec2]| -> Vec<Point> {
            centre
                .iter()
                .zip(&frames)
                .zip(offsets)
                .map(|((c, t), o)| *c + *t * o.x + t.turn_90() * o.y)
                .collect()
        };
        let (l, r) = (place(left), place(right));
        let m = l.len();
        let mut ring: Vec<Point> = l;
        ring.extend(r[1..m.saturating_sub(1)].iter().rev());
        let kept = crate::edit::douglas_peucker(&ring, true, self.step * 0.25);
        let mut out = BezPath::new();
        crate::edit::rebuild_smooth(&mut out, &kept, true);
        out
    }
}

/// The one closed outline of `path`, flattened; `None` for a path of more
/// than one — a ring with a hole, or strokes fused into a knot.
fn single_ring(path: &BezPath) -> Option<Vec<Point>> {
    let bb = path.bounding_box();
    let accuracy = (bb.width().hypot(bb.height()) / 4000.0).clamp(0.01, 0.25);
    let mut rings: Vec<Vec<Point>> = Vec::new();
    kurbo::flatten(path.iter(), accuracy, |el| match el {
        PathEl::MoveTo(p) => rings.push(vec![p]),
        PathEl::LineTo(p) => {
            if let Some(ring) = rings.last_mut()
                && ring.last() != Some(&p)
            {
                ring.push(p);
            }
        }
        _ => {}
    });
    rings.retain(|r| r.len() > 2);
    let [mut ring] = <[Vec<Point>; 1]>::try_from(rings).ok()?;
    if ring.first() == ring.last() {
        ring.pop();
    }
    Some(ring)
}

fn ring_length(ring: &[Point]) -> f64 {
    let n = ring.len();
    (0..n).map(|i| (ring[(i + 1) % n] - ring[i]).hypot()).sum()
}

fn ring_area(ring: &[Point]) -> f64 {
    let n = ring.len();
    0.5 * (0..n).map(|i| ring[i].to_vec2().cross(ring[(i + 1) % n].to_vec2())).sum::<f64>()
}

fn polyline_length(points: &[Point]) -> f64 {
    points.windows(2).map(|w| (w[1] - w[0]).hypot()).sum()
}

/// Points every `step` round a closed outline.
fn resample_ring(ring: &[Point], step: f64) -> Vec<Point> {
    let n = ring.len();
    let mut out = vec![ring[0]];
    let mut carried = 0.0;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let length = (b - a).hypot();
        let mut along = step - carried;
        while along < length {
            out.push(a.lerp(b, along / length));
            along += step;
        }
        carried = length - (along - step);
    }
    // The last point lands on or just short of where the ring began.
    if out.len() > 1 && (out[out.len() - 1] - out[0]).hypot() < step * 0.5 {
        out.pop();
    }
    out
}

/// `count` points along an open polyline at even shares of its length, its
/// ends included.
fn resample_open(points: &[Point], count: usize) -> Vec<Point> {
    let total = polyline_length(points);
    if count < 2 || points.len() < 2 || !(total > 0.0) {
        return vec![points[0]; count.max(1)];
    }
    let mut out = Vec::with_capacity(count);
    let mut seg = 0;
    let mut seg_start = 0.0;
    for k in 0..count {
        let target = total * k as f64 / (count - 1) as f64;
        while seg + 2 < points.len() && seg_start + (points[seg + 1] - points[seg]).hypot() < target {
            seg_start += (points[seg + 1] - points[seg]).hypot();
            seg += 1;
        }
        let length = (points[seg + 1] - points[seg]).hypot();
        let f = if length > 0.0 { ((target - seg_start) / length).clamp(0.0, 1.0) } else { 0.0 };
        out.push(points[seg].lerp(points[seg + 1], f));
    }
    out
}

/// The direction of travel at each point of a line, from its neighbours.
fn frames_along(points: &[Point]) -> Vec<Vec2> {
    let n = points.len();
    let mut last = Vec2::new(1.0, 0.0);
    (0..n)
        .map(|i| {
            let d = points[(i + 1).min(n - 1)] - points[i.saturating_sub(1)];
            let length = d.hypot();
            if length > 1e-12 {
                last = d / length;
            }
            last
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrushProfile, BrushSpine, StrokeSample};
    use kurbo::{Circle, Rect};

    /// The outline a fluid brush leaves for a 400-wide arc sagging 60, drawn
    /// with a 4-unit wobble — and then nothing else: no spine.
    fn wobbly_stroke_outline() -> BezPath {
        let samples: Vec<StrokeSample> = (0..=600)
            .map(|i| {
                let t = f64::from(i) / 600.0;
                let x = 400.0 * t;
                StrokeSample {
                    point: Point::new(
                        x,
                        -60.0 * (std::f64::consts::PI * t).sin() + 4.0 * (x / 35.0 * std::f64::consts::TAU).sin(),
                    ),
                    pressure: 1.0,
                    time: t * 0.6,
                }
            })
            .collect();
        BrushSpine::new(samples, BrushProfile { width: 10.0, ..Default::default() }).outline()
    }

    /// How far the middle strays from the arc it was meant to be.
    fn off_the_arc(centre: &[Point]) -> f64 {
        let sum: f64 = centre
            .iter()
            .map(|p| (p.y + 60.0 * (std::f64::consts::PI * p.x / 400.0).sin()).powi(2))
            .sum();
        (sum / centre.len() as f64).sqrt()
    }

    #[test]
    fn a_stroke_outline_has_a_middle_and_a_blob_does_not() {
        let fill = FillSpine::from_outline(&wobbly_stroke_outline()).expect("a stroke");
        let (a, b) = (fill.centre()[0], *fill.centre().last().unwrap());
        let tips = [a.x.min(b.x), a.x.max(b.x)];
        assert!(tips[0] < 5.0 && tips[1] > 395.0, "tip to tip: {tips:?}");
        assert!(off_the_arc(fill.centre()) < 4.0, "{}", off_the_arc(fill.centre()));

        // The outline it rebuilds, left alone, is the outline it came from.
        let source = wobbly_stroke_outline();
        let again = fill.outline_along(fill.centre());
        assert!((again.area().abs() / source.area().abs() - 1.0).abs() < 0.03);

        assert!(FillSpine::from_outline(&Circle::new((0.0, 0.0), 50.0).to_path(0.1)).is_none());
        assert!(FillSpine::from_outline(&Rect::new(0.0, 0.0, 100.0, 80.0).to_path(0.1)).is_none());
        let mut ring = Circle::new((0.0, 0.0), 50.0).to_path(0.1);
        ring.extend(Circle::new((0.0, 0.0), 40.0).to_path(0.1));
        assert!(FillSpine::from_outline(&ring).is_none(), "a ring with a hole is not one stroke");
    }

    /// **Every press does more**: smoothing a spineless stroke three times
    /// takes the wobble down each time, and straightening it three times
    /// brings it closer to its corners each time.
    #[test]
    fn every_press_of_a_spineless_stroke_goes_further() {
        let mut path = wobbly_stroke_outline();
        let raw = off_the_arc(FillSpine::from_outline(&path).unwrap().centre());
        for press in 1..=3 {
            let next = FillSpine::from_outline(&path).expect("still a stroke").smoother(0.2);
            assert!(next != path, "press {press} changed nothing");
            path = next;
        }
        let now = off_the_arc(FillSpine::from_outline(&path).unwrap().centre());
        assert!(now < raw * 0.3, "three presses left {now} of {raw}");

        // A stroke meant to be straight, drawn shaky: every press of
        // Straighten takes a share of what is left of the shake.
        let samples: Vec<StrokeSample> = (0..=600)
            .map(|i| {
                let t = f64::from(i) / 600.0;
                let x = 400.0 * t;
                StrokeSample {
                    point: Point::new(x, 5.0 * (x / 60.0 * std::f64::consts::TAU).sin()),
                    pressure: 1.0,
                    time: t * 0.6,
                }
            })
            .collect();
        let mut path = BrushSpine::new(samples, BrushProfile { width: 10.0, ..Default::default() }).outline();
        let shake = |p: &BezPath| {
            let c = FillSpine::from_outline(p).expect("still a stroke").centre().to_vec();
            let (a, b) = (c[0], *c.last().unwrap());
            c.iter().map(|q| ((b - a).cross(*q - a) / (b - a).hypot()).abs()).fold(0.0, f64::max)
        };
        let mut last = shake(&path);
        for press in 1..=3 {
            path = FillSpine::from_outline(&path).unwrap().straighter(0.5);
            let now = shake(&path);
            assert!(now < last * 0.8, "press {press}: {now} from {last}");
            last = now;
        }
    }
}
