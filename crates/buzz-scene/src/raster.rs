//! Soft-edged raster painting: a brush that fades at its edge.
//!
//! # Why a raster brush at all, in a vector application
//!
//! Every other brush here produces geometry — an outline filled with a colour,
//! which scales to any zoom and stays editable. That is the right answer for
//! almost everything, and it is exactly the wrong answer for a *soft* edge. A
//! gradient that fades to nothing along the silhouette of a wandering stroke is
//! not a shape with a colour; it is a different opacity at every point of a
//! two-dimensional region. Approximating it with concentric outlines is what
//! `buzz_fx`'s blur does, and it costs a boolean per band.
//!
//! So a soft brush paints **pixels**, and the pixels become a bitmap, and the
//! bitmap is a fill — which means the stroke it makes is an ordinary shape that
//! the Lasso can cut, the Magic Wand can pick from, a mask can hole and a tween
//! can move. Nothing downstream knows it was painted rather than drawn.
//!
//! # What is stored while painting
//!
//! **Coverage, one byte a pixel — not colour.** A stroke is one colour, so the
//! only thing that varies across it is how much of that colour each pixel got.
//! Keeping coverage alone is a quarter of the memory, and it makes the rule
//! that matters trivial: overlapping stamps within one stroke take the
//! **greater** coverage rather than adding. Adding is what produces the beaded
//! string of dark blobs that gives away a naive brush; Photoshop separates
//! *flow* from *opacity* for the same reason.

use buzz_geom::{Point, Rect};
use peniko::Color;

use crate::image::{ImageAsset, ImageId};

/// A round brush with a soft edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoftBrush {
    /// Radius in document units.
    pub radius: f64,
    /// Where the fade begins, as a fraction of the radius.
    ///
    /// `1.0` is a hard edge, `0.0` fades from the very centre. Photoshop's
    /// Hardness, and the same numbers.
    pub hardness: f64,
    /// How much colour the stroke lays down, `0.0`–`1.0`.
    pub flow: f64,
    pub color: Color,
}

impl Default for SoftBrush {
    fn default() -> Self {
        Self {
            radius: 12.0,
            hardness: 0.5,
            flow: 1.0,
            color: Color::BLACK,
        }
    }
}

impl SoftBrush {
    /// How much of the colour lands `distance` from the centre of a stamp.
    ///
    /// Smoothstep rather than a straight ramp: a linear fade has a visible
    /// crease where it meets the flat middle and another where it reaches
    /// nothing, and the eye finds both.
    fn coverage_at(&self, distance: f64) -> f64 {
        let radius = self.radius.max(f64::MIN_POSITIVE);
        let t = distance / radius;
        if t >= 1.0 {
            return 0.0;
        }
        // Always at least one pixel of fade, however hard the brush: a disc
        // with no falloff at all is a staircase, and a staircase reads as a
        // fault rather than as a hard edge.
        let softest = (1.0 / radius).min(0.5);
        let hardness = self.hardness.clamp(0.0, 1.0).min(1.0 - softest);
        if t <= hardness {
            return 1.0;
        }
        let u = ((t - hardness) / (1.0 - hardness)).clamp(0.0, 1.0);
        // 1 - smoothstep(u)
        1.0 - u * u * (3.0 - 2.0 * u)
    }

    /// Distance between stamps along the stroke, in document units.
    ///
    /// A quarter of the radius. Wider and the stroke scallops; closer costs
    /// more for a difference nobody can see, since each stamp only ever raises
    /// coverage to its own maximum.
    fn spacing(&self) -> f64 {
        (self.radius * 0.25).max(0.35)
    }
}

/// No stroke's buffer may exceed this many pixels.
///
/// Sixteen megapixels — a canvas 4 000 across. A brush is dragged by hand, so
/// the only way past this is a deliberate sweep across an enormous document at
/// an enormous size; the alternative to a limit is an allocation the size of
/// the gesture, and a slip of the wrist should not exhaust memory.
const MAX_CANVAS_PIXELS: u64 = 16_000_000;

/// A coverage buffer sitting somewhere in document space.
///
/// One pixel per document unit — the resolution the film exports at, so paint
/// finer than this would be invisible in the result.
#[derive(Debug, Clone, PartialEq)]
pub struct Canvas {
    /// Document position of the top-left corner of pixel `(0, 0)`.
    origin: Point,
    width: u32,
    height: u32,
    /// `0`–`255` per pixel.
    coverage: Vec<u8>,
}

impl Canvas {
    /// An empty canvas covering `area`, rounded out to whole pixels.
    pub fn covering(area: Rect) -> Self {
        let x0 = area.x0.floor();
        let y0 = area.y0.floor();
        let width = ((area.x1.ceil() - x0).max(1.0)) as u64;
        let height = ((area.y1.ceil() - y0).max(1.0)) as u64;
        let (width, height) = clamp_size(width, height);
        Self {
            origin: Point::new(x0, y0),
            width: width as u32,
            height: height as u32,
            coverage: vec![0; (width * height) as usize],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Where this canvas sits in document space.
    pub fn area(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + f64::from(self.width),
            self.origin.y + f64::from(self.height),
        )
    }

    /// Coverage at a pixel, `0`–`255`. Outside is zero.
    pub fn coverage_at(&self, x: i64, y: i64) -> u8 {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return 0;
        }
        self.coverage[(y as usize) * self.width as usize + x as usize]
    }

    /// Is there any paint on this at all?
    pub fn is_blank(&self) -> bool {
        self.coverage.iter().all(|c| *c == 0)
    }

    /// One stamp of the brush, centred on a document point.
    ///
    /// Coverage is raised, never added: two stamps that overlap leave the
    /// deeper of the two, which is what keeps a slow-drawn stroke from beading.
    pub fn stamp(&mut self, centre: Point, brush: &SoftBrush) {
        let r = brush.radius.max(f64::MIN_POSITIVE);
        let cx = centre.x - self.origin.x;
        let cy = centre.y - self.origin.y;

        let x0 = ((cx - r).floor() as i64).max(0);
        let x1 = ((cx + r).ceil() as i64).min(i64::from(self.width));
        let y0 = ((cy - r).floor() as i64).max(0);
        let y1 = ((cy + r).ceil() as i64).min(i64::from(self.height));

        for y in y0..y1 {
            let dy = (y as f64) + 0.5 - cy;
            let row = (y as usize) * self.width as usize;
            for x in x0..x1 {
                let dx = (x as f64) + 0.5 - cx;
                let d = (dx * dx + dy * dy).sqrt();
                let c = brush.coverage_at(d);
                if c <= 0.0 {
                    continue;
                }
                let value = (c * 255.0).round().clamp(0.0, 255.0) as u8;
                let slot = &mut self.coverage[row + x as usize];
                if value > *slot {
                    *slot = value;
                }
            }
        }
    }

    /// Stamp along a run of points, at the brush's own spacing.
    ///
    /// The points are the pointer's path; the stamps are laid at even
    /// intervals along it, so how fast the pointer moved makes no difference to
    /// how solid the line is. Without that, a quick stroke is a dotted line.
    pub fn stroke(&mut self, points: &[Point], brush: &SoftBrush) {
        let Some(&first) = points.first() else { return };
        if points.len() == 1 {
            self.stamp(first, brush);
            return;
        }
        let spacing = brush.spacing();
        self.stamp(first, brush);
        let mut carried = 0.0;
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let length = (b - a).hypot();
            if length <= f64::MIN_POSITIVE {
                continue;
            }
            let mut travelled = spacing - carried;
            while travelled <= length {
                let t = travelled / length;
                self.stamp(a.lerp(b, t), brush);
                travelled += spacing;
            }
            carried = (length - (travelled - spacing)).max(0.0);
        }
        // The very end always gets a stamp, so a stroke finishes where the
        // pointer did rather than up to one spacing short of it.
        if let Some(&last) = points.last() {
            self.stamp(last, brush);
        }
    }

    /// A canvas holding one stroke, sized to the stroke.
    ///
    /// `None` for a gesture with nothing in it. The buffer is exactly the
    /// stroke's own extent plus its radius — not the stage — so painting costs
    /// what was painted rather than what it was painted on.
    pub fn for_stroke(points: &[Point], brush: &SoftBrush) -> Option<Self> {
        let mut bounds: Option<Rect> = None;
        for p in points {
            let r = Rect::new(p.x, p.y, p.x, p.y);
            bounds = Some(match bounds {
                Some(b) => b.union(r),
                None => r,
            });
        }
        // One pixel of margin past the radius, so the outermost fade is not
        // clipped by the edge of its own buffer.
        let margin = brush.radius.max(0.0) + 1.0;
        let area = bounds?.inflate(margin, margin);
        let mut canvas = Self::covering(area);
        canvas.stroke(points, brush);
        Some(canvas)
    }

    /// The bitmap this canvas paints: the brush's colour, at this coverage.
    ///
    /// Straight alpha, as the rest of the image path stores it.
    pub fn to_asset(&self, id: ImageId, name: impl Into<String>, brush: &SoftBrush) -> ImageAsset {
        let rgba = brush.color.to_rgba8().to_u8_array();
        let flow = brush.flow.clamp(0.0, 1.0) * f64::from(rgba[3]) / 255.0;
        let mut pixels = Vec::with_capacity(self.coverage.len() * 4);
        for c in &self.coverage {
            let alpha = (f64::from(*c) * flow).round().clamp(0.0, 255.0) as u8;
            pixels.extend_from_slice(&[rgba[0], rgba[1], rgba[2], alpha]);
        }
        // Painted, not read from a file: the pixels *are* the original, so
        // there is nothing to keep beside them. Saving encodes a PNG.
        let mut asset = ImageAsset::from_pixels(
            id,
            name,
            self.width,
            self.height,
            std::sync::Arc::new(pixels),
        );
        asset.painted = true;
        asset
    }
}

/// One bitmap covering two, and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedPaint {
    /// Document position of the top-left corner of pixel `(0, 0)`.
    pub origin: Point,
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8, `width * height * 4` long.
    pub pixels: Vec<u8>,
}

impl MergedPaint {
    /// Where this sits in document space.
    pub fn area(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + f64::from(self.width),
            self.origin.y + f64::from(self.height),
        )
    }
}

/// **Paint a stroke into paint already there**, giving one bitmap covering both.
///
/// # Why this exists
///
/// Animate's merge drawing model is that paint of one colour is *one thing*:
/// draw over what you drew and there is one shape afterwards, not two stacked.
/// A soft brush had no such rule — every stroke became its own bitmap on its own
/// shape — so a face built out of forty strokes was forty objects, and selecting
/// "the paint" meant selecting forty things and hoping.
///
/// # It must not change the picture
///
/// The two are composited **source-over**, which is exactly what stacking the
/// two shapes already showed. Fusing therefore collapses the object count and
/// leaves the frame identical, which is the only way it can be safe to do
/// without asking. (Within a single stroke coverage is *raised* rather than
/// composited — see [`Canvas::stamp`] — because that is one pass of one brush;
/// two strokes really are two passes, and a second pass over a soft edge does
/// darken it.)
///
/// `under_origin` is where the existing bitmap's first pixel sits in document
/// space; the caller is responsible for having checked that it is still one
/// pixel to one document unit and square to the axes, because this does no
/// resampling. `None` if the union would be larger than a canvas may be.
pub fn merge_over(
    under: &ImageAsset,
    under_origin: Point,
    canvas: &Canvas,
    brush: &SoftBrush,
) -> Option<MergedPaint> {
    let under_area = Rect::new(
        under_origin.x,
        under_origin.y,
        under_origin.x + f64::from(under.width),
        under_origin.y + f64::from(under.height),
    );
    let area = under_area.union(canvas.area());

    let origin = Point::new(area.x0.floor(), area.y0.floor());
    let width = (area.x1.ceil() - origin.x).max(1.0) as u64;
    let height = (area.y1.ceil() - origin.y).max(1.0) as u64;
    if width * height > MAX_CANVAS_PIXELS {
        return None;
    }
    let (width, height) = (width as u32, height as u32);

    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];

    // The existing paint, moved into the union's frame. Both are pixel-aligned,
    // so this is a copy rather than a resample.
    let under_dx = (under_origin.x - origin.x).round() as i64;
    let under_dy = (under_origin.y - origin.y).round() as i64;
    for y in 0..under.height as i64 {
        for x in 0..under.width as i64 {
            let (dx, dy) = (x + under_dx, y + under_dy);
            if dx < 0 || dy < 0 || dx >= i64::from(width) || dy >= i64::from(height) {
                continue;
            }
            let from = ((y * i64::from(under.width) + x) * 4) as usize;
            let to = ((dy * i64::from(width) + dx) * 4) as usize;
            pixels[to..to + 4].copy_from_slice(&under.pixels[from..from + 4]);
        }
    }

    // Then the new stroke over it.
    let ink = brush.color.to_rgba8().to_u8_array();
    let flow = brush.flow.clamp(0.0, 1.0) * f64::from(ink[3]) / 255.0;
    let canvas_dx = (canvas.area().x0 - origin.x).round() as i64;
    let canvas_dy = (canvas.area().y0 - origin.y).round() as i64;
    for y in 0..canvas.height() as i64 {
        for x in 0..canvas.width() as i64 {
            let coverage = canvas.coverage_at(x, y);
            if coverage == 0 {
                continue;
            }
            let (dx, dy) = (x + canvas_dx, y + canvas_dy);
            if dx < 0 || dy < 0 || dx >= i64::from(width) || dy >= i64::from(height) {
                continue;
            }
            let alpha = f64::from(coverage) * flow / 255.0;
            let to = ((dy * i64::from(width) + dx) * 4) as usize;
            let below = f64::from(pixels[to + 3]) / 255.0;
            // Straight alpha, source-over. The colours are the same, so only
            // the alpha has anywhere to go — but it is written in full rather
            // than assumed, so a later merge of two inks is a change of one
            // line here rather than a rewrite.
            let out = alpha + below * (1.0 - alpha);
            pixels[to] = ink[0];
            pixels[to + 1] = ink[1];
            pixels[to + 2] = ink[2];
            pixels[to + 3] = (out * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    Some(MergedPaint {
        origin,
        width,
        height,
        pixels,
    })
}

/// **The one colour a painted bitmap is in**, if it has any paint at all.
///
/// Every pixel of a painted bitmap carries the brush's colour and differs only
/// in coverage, so the first pixel that is not fully transparent answers for all
/// of them. `None` for a bitmap with nothing on it.
pub fn painted_ink(asset: &ImageAsset) -> Option<Color> {
    asset
        .pixels
        .chunks_exact(4)
        .find(|px| px[3] > 0)
        .map(|px| Color::from_rgba8(px[0], px[1], px[2], 255))
}

/// Keep a canvas within the pixel limit, shrinking the larger side first.
fn clamp_size(width: u64, height: u64) -> (u64, u64) {
    let (mut w, mut h) = (width.max(1), height.max(1));
    while w * h > MAX_CANVAS_PIXELS {
        if w >= h {
            w = (w / 2).max(1);
        } else {
            h = (h / 2).max(1);
        }
    }
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brush(radius: f64, hardness: f64) -> SoftBrush {
        SoftBrush {
            radius,
            hardness,
            flow: 1.0,
            color: Color::BLACK,
        }
    }

    /// **A soft brush fades.** The whole request, in one assertion: coverage
    /// falls away from the middle of a stamp instead of stopping dead.
    #[test]
    fn a_soft_stamp_fades_from_its_middle_to_nothing() {
        let mut canvas = Canvas::covering(Rect::new(0.0, 0.0, 100.0, 100.0));
        canvas.stamp(Point::new(50.0, 50.0), &brush(20.0, 0.0));

        let middle = canvas.coverage_at(50, 50);
        let half = canvas.coverage_at(60, 50);
        let edge = canvas.coverage_at(69, 50);
        let outside = canvas.coverage_at(75, 50);

        // Not exactly 255: with hardness 0 the fade begins at the very centre,
        // and this pixel's own middle is half a unit off it.
        assert!(
            middle >= 250,
            "the middle of a stamp is solid, not {middle}"
        );
        assert!(
            half > 20 && half < 235,
            "halfway out should be partly covered, and is {half}"
        );
        assert!(edge < half, "and it keeps falling: {edge} after {half}");
        assert_eq!(outside, 0, "past the radius there is nothing");
    }

    /// Hardness is the dial that makes it a hard brush again.
    #[test]
    fn hardness_moves_where_the_fade_begins() {
        let mut soft = Canvas::covering(Rect::new(0.0, 0.0, 100.0, 100.0));
        soft.stamp(Point::new(50.0, 50.0), &brush(20.0, 0.0));
        let mut hard = Canvas::covering(Rect::new(0.0, 0.0, 100.0, 100.0));
        hard.stamp(Point::new(50.0, 50.0), &brush(20.0, 1.0));

        // Three quarters of the way out: a hard brush is still solid there, a
        // fully soft one is well on its way to nothing.
        let at = |c: &Canvas| c.coverage_at(65, 50);
        assert_eq!(at(&hard), 255, "a hard brush is solid to its edge");
        assert!(
            at(&soft) < 128,
            "a fully soft brush is more than half gone by then, not {}",
            at(&soft)
        );

        // But even the hardest brush fades within its last pixel, or its edge
        // is a staircase.
        assert!(
            hard.coverage_at(70, 50) < 255,
            "a hard edge still needs its one pixel of fade"
        );
    }

    /// **Overlapping stamps do not bead.**
    ///
    /// The failure this prevents is the classic one: a slowly drawn stroke
    /// coming out as a string of dark blobs where the stamps piled up. Two
    /// stamps on the same spot must leave what one leaves.
    #[test]
    fn stamping_twice_in_one_place_is_no_darker_than_once() {
        let mut once = Canvas::covering(Rect::new(0.0, 0.0, 60.0, 60.0));
        once.stamp(Point::new(30.0, 30.0), &brush(10.0, 0.2));

        let mut twice = Canvas::covering(Rect::new(0.0, 0.0, 60.0, 60.0));
        twice.stamp(Point::new(30.0, 30.0), &brush(10.0, 0.2));
        twice.stamp(Point::new(30.0, 30.0), &brush(10.0, 0.2));

        for y in 0..60 {
            for x in 0..60 {
                assert_eq!(
                    once.coverage_at(x, y),
                    twice.coverage_at(x, y),
                    "coverage grew at ({x}, {y}) when the same stamp was laid twice"
                );
            }
        }
    }

    /// A stroke is even along its length, however fast the pointer moved.
    #[test]
    fn a_stroke_lays_paint_evenly_whatever_the_pointer_did() {
        // Two points far apart — one quick flick — and many points along the
        // same line, drawn slowly. The line must come out the same.
        let b = brush(8.0, 0.5);
        let quick =
            Canvas::for_stroke(&[Point::new(20.0, 50.0), Point::new(180.0, 50.0)], &b).unwrap();

        let slow_points: Vec<Point> = (0..=160)
            .map(|i| Point::new(20.0 + f64::from(i), 50.0))
            .collect();
        let slow = Canvas::for_stroke(&slow_points, &b).unwrap();

        assert_eq!(
            quick.area(),
            slow.area(),
            "the same stroke covers the same ground"
        );
        // Sample down the middle of the line: solid all the way, both times.
        for x in 25..175 {
            let (qx, qy) = (x - quick.area().x0 as i64, 50 - quick.area().y0 as i64);
            assert_eq!(
                quick.coverage_at(qx, qy),
                255,
                "the quick stroke is thin at x={x}"
            );
            assert_eq!(
                slow.coverage_at(qx, qy),
                255,
                "the slow stroke differs from the quick one at x={x}"
            );
        }
    }

    /// A stroke's buffer is the size of the stroke, not the size of the stage.
    #[test]
    fn a_stroke_allocates_only_what_it_covers() {
        let b = brush(10.0, 0.5);
        let canvas =
            Canvas::for_stroke(&[Point::new(500.0, 500.0), Point::new(560.0, 500.0)], &b).unwrap();

        assert!(
            canvas.width() < 100 && canvas.height() < 40,
            "a short stroke took a {}x{} buffer",
            canvas.width(),
            canvas.height()
        );
        let area = canvas.area();
        assert!(
            area.contains(Point::new(500.0, 500.0)) && area.contains(Point::new(560.0, 500.0)),
            "and it does not cover the stroke: {area:?}"
        );
    }

    /// The bitmap it makes is the brush's colour at the coverage painted.
    #[test]
    fn the_bitmap_carries_the_colour_at_the_coverage_painted() {
        let mut b = brush(20.0, 0.0);
        b.color = Color::from_rgb8(0xFF, 0x00, 0x00);
        let mut canvas = Canvas::covering(Rect::new(0.0, 0.0, 100.0, 100.0));
        canvas.stamp(Point::new(50.0, 50.0), &b);

        let asset = canvas.to_asset(ImageId(1), "Stroke", &b);
        let middle = asset.pixel(50, 50);
        assert_eq!([middle[0], middle[1], middle[2]], [255, 0, 0]);
        assert!(middle[3] >= 250, "solid in the middle, not {}", middle[3]);

        let out = asset.pixel(80, 50);
        assert_eq!(out[3], 0, "and transparent outside, not white");

        let fading = asset.pixel(62, 50);
        assert!(
            fading[3] > 0 && fading[3] < 255,
            "the edge should be part-transparent, and is alpha {}",
            fading[3]
        );
        assert_eq!(
            [fading[0], fading[1], fading[2]],
            [255, 0, 0],
            "a fading edge keeps the colour and loses the alpha \u{2014} straight alpha, \
             so the colour under a transparent pixel must not go grey"
        );
    }

    /// Flow thins the whole stroke without changing its shape.
    #[test]
    fn flow_thins_the_paint_without_moving_its_edge() {
        let mut full = brush(20.0, 0.5);
        full.flow = 1.0;
        let mut half = full;
        half.flow = 0.5;

        let mut canvas = Canvas::covering(Rect::new(0.0, 0.0, 100.0, 100.0));
        canvas.stamp(Point::new(50.0, 50.0), &full);

        let a = canvas.to_asset(ImageId(1), "a", &full);
        let b = canvas.to_asset(ImageId(2), "b", &half);
        assert_eq!(a.pixel(50, 50)[3], 255);
        assert_eq!(b.pixel(50, 50)[3], 128, "half the flow, half the paint");
        assert_eq!(a.pixel(80, 50)[3], 0);
        assert_eq!(b.pixel(80, 50)[3], 0, "and the edge has not moved");
    }

    /// A wild sweep does not allocate the machine's memory.
    #[test]
    fn an_enormous_stroke_is_capped_rather_than_allocated() {
        let b = brush(50.0, 0.5);
        let canvas =
            Canvas::for_stroke(&[Point::new(0.0, 0.0), Point::new(60_000.0, 60_000.0)], &b)
                .unwrap();
        let pixels = u64::from(canvas.width()) * u64::from(canvas.height());
        assert!(
            pixels <= MAX_CANVAS_PIXELS,
            "a huge sweep asked for {pixels} pixels"
        );
    }

    /// **A painted stroke is prompt.** It is rebuilt on every pointer move to
    /// preview it, so its cost is paid sixty times a second, not once.
    #[test]
    fn a_long_stroke_paints_quickly_enough_to_preview() {
        let b = brush(40.0, 0.4);
        // A sweep right across a 1080p stage, sampled as a pointer would.
        let points: Vec<Point> = (0..=600)
            .map(|i| {
                let t = f64::from(i) / 600.0;
                Point::new(100.0 + t * 1700.0, 540.0 + (t * 12.0).sin() * 200.0)
            })
            .collect();

        let start = std::time::Instant::now();
        let canvas = Canvas::for_stroke(&points, &b).expect("a stroke");
        let took = start.elapsed();

        assert!(!canvas.is_blank());
        assert!(
            took.as_millis() < 40,
            "a full-width stroke took {took:?} to build"
        );
        eprintln!(
            "raster stroke across a 1080p stage: {took:?} for {}x{}",
            canvas.width(),
            canvas.height()
        );
    }
}
