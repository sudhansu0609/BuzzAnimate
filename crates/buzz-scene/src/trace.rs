//! **Tracing a picture into artwork you can draw on** — Animate's Trace Bitmap.
//!
//! # What this is for
//!
//! A photograph, a scan of a pencil test, a sketch somebody sent — all of them
//! arrive as pixels, and pixels are the one thing in this program you cannot
//! bucket-fill, reshape, tween or recolour. Tracing turns them into ordinary
//! shapes: paths with fills, on a layer, exactly like something drawn with the
//! brush. What comes out is editable, which is the whole point of doing it.
//!
//! # Why it reuses the paint bucket's machinery
//!
//! The bucket has been solving most of this since it was written. "Which
//! pixels belong to this region, and what is the outline of that set" is
//! exactly what a flood fill does, and [`crate::bucket`] already rasterises,
//! floods, traces the boundary into loops and simplifies the staircase the
//! pixel grid leaves behind. Tracing is that same pipeline with a different
//! question at the front: instead of *which pixels did the user click into*,
//! it is *which pixels are this colour*.
//!
//! Keeping one contour tracer behind both is why a traced outline and a bucket
//! fill land on exactly the same pixel boundaries, and why fixing a staircase
//! in one fixes it in the other.
//!
//! # The three steps
//!
//! 1. **Quantise** the picture to a small palette ([`quantise`]), by median
//!    cut. Two colours is line art; a dozen is a poster.
//! 2. **Find the regions** of each palette colour, and throw away the specks.
//! 3. **Trace, simplify and smooth** each region's outline into a path.
//!
//! # What it is not
//!
//! It is not a line-*following* tracer: it does not find a pencil stroke and
//! give you a stroked path down its middle. It finds *areas* and gives you
//! their outlines, which is what Animate does and what makes the result
//! paintable. A line traced this way comes back as a long thin filled shape —
//! which is what a brush stroke already is in this program, so it behaves like
//! one.

use std::sync::Arc;

use crate::bucket::{Grid, simplify, trace_contours};
use crate::object::ShapeData;
use buzz_geom::{BezPath, FillMode, Point};
use peniko::Color;

/// How to trace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceOptions {
    /// **How many colours to reduce the picture to**, 2 to 64.
    ///
    /// Two is line art: ink and paper. Six to twelve suits a cartoon or a flat
    /// illustration. Going much past that stops being a tracing and starts
    /// being a mosaic, with a shape for every gradient step.
    pub colours: usize,
    /// **How much of the pixel staircase to keep**, in pixels.
    ///
    /// The Douglas–Peucker tolerance. Below about `0.5` every jag of the grid
    /// survives and the result has thousands of points; above about `4` a face
    /// starts losing its features. `1.5` is a good drawing.
    pub detail: f64,
    /// **How much to round the corners off**, `0` to `1`.
    ///
    /// A traced outline is a polygon, and a polygon reads as *traced*. This
    /// pulls it into curves. Too much and everything becomes a blob, which is
    /// why it is a dial rather than something applied always.
    pub smooth: f64,
    /// **The smallest region worth keeping**, in pixels of area.
    ///
    /// A photograph quantised to eight colours produces thousands of one-pixel
    /// islands along every edge. They are not artwork, they are dither, and
    /// left in they make a document nothing can open quickly.
    pub speckle: usize,
    /// **Throw away the colour that covers the most of the picture.**
    ///
    /// For line art this is the paper, and dropping it is the difference
    /// between outlines you can paint inside and outlines sitting on an opaque
    /// white rectangle. For a photograph it is usually the sky, and you
    /// probably do not want it dropped — so it is a choice, not a rule.
    pub drop_background: bool,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            colours: 6,
            detail: 1.5,
            smooth: 0.5,
            speckle: 16,
            drop_background: false,
        }
    }
}

impl TraceOptions {
    /// **Ink and paper**: two colours, the paper dropped, tuned for a scan of a
    /// drawing. What you want when the plan is to trace and then paint inside.
    pub fn line_art() -> Self {
        Self {
            colours: 2,
            detail: 1.2,
            smooth: 0.6,
            speckle: 12,
            drop_background: true,
        }
    }
}

/// What a trace produced, and what it had to leave out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TraceReport {
    /// The artwork, back to front — the largest regions first, so the picture
    /// assembles the way it would have been painted.
    pub shapes: Vec<ShapeData>,
    /// Regions kept.
    pub regions: usize,
    /// Regions thrown away for being smaller than `speckle`.
    pub specks: usize,
    /// What to put in the status bar.
    pub message: String,
}

/// **Trace `pixels` into shapes**, in pixel coordinates.
///
/// `pixels` is straight RGBA8, `width * height * 4` long — an
/// [`crate::ImageAsset`]'s own buffer. The paths come back in **pixel space**,
/// with `(0, 0)` at the top left: the caller knows where on the stage the
/// picture sits and at what size, and this does not.
///
/// Fully transparent pixels are not traced at all, so a cut-out arrives as a
/// cut-out rather than as a rectangle with a hole worked out of it.
pub fn trace(width: u32, height: u32, pixels: &[u8], options: &TraceOptions) -> TraceReport {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return TraceReport {
            message: "There is nothing in that picture to trace".into(),
            ..Default::default()
        };
    }

    let colours = options.colours.clamp(2, 64);
    let palette = quantise(pixels, colours);
    if palette.is_empty() {
        return TraceReport {
            message: "That picture is entirely transparent".into(),
            ..Default::default()
        };
    }

    // Every pixel's palette entry, or `None` where it is transparent.
    let mut index: Vec<Option<u8>> = Vec::with_capacity(w * h);
    let mut counts = vec![0usize; palette.len()];
    for i in 0..w * h {
        let px = &pixels[i * 4..i * 4 + 4];
        if px[3] < 128 {
            index.push(None);
            continue;
        }
        let n = nearest(&palette, [px[0], px[1], px[2]]);
        counts[n] += 1;
        index.push(Some(n as u8));
    }

    // The background is the colour covering the most of the picture, which for
    // a scanned drawing is the paper.
    let background = options.drop_background.then(|| {
        counts
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| **n)
            .map(|(i, _)| i as u8)
            .unwrap_or(0)
    });

    let mut regions: Vec<(usize, ShapeData)> = Vec::new();
    let mut specks = 0usize;
    // One scratch grid for the whole run: a picture can have thousands of
    // regions and one allocation the size of the image per region is the
    // difference between a trace that takes a second and one that takes a
    // minute.
    let mut scratch = Grid::new(w, h);
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let mut cells: Vec<usize> = Vec::new();

    for start in 0..w * h {
        if seen[start] {
            continue;
        }
        let Some(colour) = index[start] else {
            seen[start] = true;
            continue;
        };
        if Some(colour) == background {
            seen[start] = true;
            continue;
        }

        // **Flood the region of this one colour**, four-connected — the same
        // connectivity the bucket floods with, so a region traced here and a
        // region filled there are the same region.
        cells.clear();
        stack.clear();
        stack.push(start);
        seen[start] = true;
        while let Some(at) = stack.pop() {
            cells.push(at);
            let (x, y) = (at % w, at / w);
            let mut push = |nx: usize, ny: usize, stack: &mut Vec<usize>, seen: &mut Vec<bool>| {
                let n = ny * w + nx;
                if !seen[n] && index[n] == Some(colour) {
                    seen[n] = true;
                    stack.push(n);
                }
            };
            if x > 0 {
                push(x - 1, y, &mut stack, &mut seen);
            }
            if x + 1 < w {
                push(x + 1, y, &mut stack, &mut seen);
            }
            if y > 0 {
                push(x, y - 1, &mut stack, &mut seen);
            }
            if y + 1 < h {
                push(x, y + 1, &mut stack, &mut seen);
            }
        }

        if cells.len() < options.speckle.max(1) {
            specks += 1;
            continue;
        }

        for &at in &cells {
            scratch.set(at % w, at / w);
        }
        let path = outline(&scratch, options);
        for &at in &cells {
            scratch.unset(at % w, at / w);
        }

        if let Some(path) = path {
            let rgb = palette[colour as usize];
            let mut shape = ShapeData::filled(path, Color::from_rgb8(rgb[0], rgb[1], rgb[2]));
            // **Non-zero, because the tracer winds holes the other way.** The
            // outer loops and the interior loops come back with opposite
            // directions, which is exactly what non-zero reads as "solid, with
            // that punched out of it".
            if let Some(fill) = shape.fill.as_mut() {
                fill.rule = FillMode::NonZero;
            }
            regions.push((cells.len(), shape));
        }
    }

    // **Largest first**, so the picture assembles back to front the way it
    // would have been painted: the big flats down, the detail over them.
    regions.sort_by(|a, b| b.0.cmp(&a.0));
    let kept = regions.len();
    let shapes: Vec<ShapeData> = regions.into_iter().map(|(_, s)| s).collect();

    let message = match (kept, specks) {
        (0, 0) => "Nothing in that picture came out as a shape".to_string(),
        (0, n) => format!("Everything in that picture was smaller than the speck size ({n} regions)"),
        (k, 0) => format!("Traced {k} shapes in {} colours", palette.len()),
        (k, n) => format!(
            "Traced {k} shapes in {} colours, and dropped {n} specks",
            palette.len()
        ),
    };

    TraceReport {
        shapes,
        regions: kept,
        specks,
        message,
    }
}

/// One region's outline, traced, simplified and smoothed.
///
/// `None` when the region had no loop long enough to be a shape — a single
/// pixel, or a one-pixel-wide thread.
fn outline(grid: &Grid, options: &TraceOptions) -> Option<BezPath> {
    let loops = trace_contours(grid);
    if loops.is_empty() {
        return None;
    }
    let detail = options.detail.max(0.0);

    let mut path = BezPath::new();
    for points in loops {
        let kept = if detail > 0.0 {
            simplify(&points, detail)
        } else {
            points
        };
        if kept.len() < 3 {
            continue;
        }
        path.move_to(Point::new(kept[0].0 as f64, kept[0].1 as f64));
        for &(x, y) in &kept[1..] {
            path.line_to(Point::new(x as f64, y as f64));
        }
        path.close_path();
    }
    if path.elements().is_empty() {
        return None;
    }

    // **A polygon reads as traced.** Pulling it into curves is what makes the
    // result look drawn rather than extracted, and it is the last step because
    // smoothing before simplifying would just round off a staircase.
    let smooth = options.smooth.clamp(0.0, 1.0);
    if smooth > 0.0 {
        path = buzz_geom::smooth(&path, smooth);
    }
    Some(path)
}

// ---------------------------------------------------------------------------
// Quantisation
// ---------------------------------------------------------------------------

/// **Reduce the picture to `want` colours, by median cut.**
///
/// # Why median cut and not k-means
///
/// It is **deterministic**. K-means starts from a random seeding and gives a
/// slightly different palette every run, which would mean tracing the same
/// picture twice produced two different documents — and re-tracing after
/// nudging a setting would reshuffle every colour in the result. Median cut
/// makes the same palette from the same pixels every time, which is worth more
/// here than the marginally better clusters.
///
/// The picture is sampled rather than read whole: a palette does not get
/// meaningfully better past a hundred thousand samples, and a large photograph
/// has twenty times that.
pub fn quantise(pixels: &[u8], want: usize) -> Vec<[u8; 3]> {
    /// Enough samples to find the colours in anything; few enough to be quick.
    const SAMPLES: usize = 100_000;

    let count = pixels.len() / 4;
    if count == 0 {
        return Vec::new();
    }
    let step = (count / SAMPLES).max(1);
    let mut colours: Vec<[u8; 3]> = Vec::new();
    for i in (0..count).step_by(step) {
        let px = &pixels[i * 4..i * 4 + 4];
        if px[3] >= 128 {
            colours.push([px[0], px[1], px[2]]);
        }
    }
    if colours.is_empty() {
        return Vec::new();
    }

    let mut boxes: Vec<Vec<[u8; 3]>> = vec![colours];
    while boxes.len() < want {
        // Split the box with the widest spread along any one channel: that is
        // the one whose colours are least alike, and so the one whose splitting
        // buys the most.
        let Some((at, channel)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .map(|(i, b)| {
                let (c, range) = widest_channel(b);
                (i, c, range)
            })
            .max_by(|a, b| a.2.cmp(&b.2))
            .map(|(i, c, _)| (i, c))
        else {
            // Every box holds one colour: there is nothing left to split, and
            // the picture genuinely has fewer colours than were asked for.
            break;
        };

        let mut group = boxes.swap_remove(at);
        group.sort_by_key(|c| c[channel]);
        let half = group.len() / 2;
        let rest = group.split_off(half);
        boxes.push(group);
        boxes.push(rest);
    }

    let mut palette: Vec<[u8; 3]> = boxes
        .iter()
        .filter(|b| !b.is_empty())
        .map(|b| {
            let n = b.len() as u64;
            let sum = b.iter().fold([0u64; 3], |mut acc, c| {
                acc[0] += c[0] as u64;
                acc[1] += c[1] as u64;
                acc[2] += c[2] as u64;
                acc
            });
            [
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
            ]
        })
        .collect();
    // A stable order, so the same picture traces to the same document.
    palette.sort();
    palette.dedup();
    palette
}

/// The channel a box of colours spreads widest along, and by how much.
fn widest_channel(colours: &[[u8; 3]]) -> (usize, u16) {
    let mut lo = [255u8; 3];
    let mut hi = [0u8; 3];
    for c in colours {
        for k in 0..3 {
            lo[k] = lo[k].min(c[k]);
            hi[k] = hi[k].max(c[k]);
        }
    }
    let mut best = (0usize, 0u16);
    for k in 0..3 {
        let range = (hi[k] - lo[k]) as u16;
        if range > best.1 {
            best = (k, range);
        }
    }
    best
}

/// The palette entry nearest `rgb`, by squared distance.
///
/// Plain RGB rather than a perceptual space: the palette came out of the same
/// numbers, so matching in the same units is what keeps a pixel assigned to
/// the box it was clustered into.
fn nearest(palette: &[[u8; 3]], rgb: [u8; 3]) -> usize {
    let mut best = (0usize, u32::MAX);
    for (i, c) in palette.iter().enumerate() {
        let d = (0..3)
            .map(|k| {
                let d = c[k] as i32 - rgb[k] as i32;
                (d * d) as u32
            })
            .sum::<u32>();
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// Keep the `Arc` import honest for callers that hand us shared pixels.
#[allow(dead_code)]
fn _shared(pixels: &Arc<Vec<u8>>) -> usize {
    pixels.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;

    /// A picture: `w` by `h`, filled by `f` returning RGBA.
    fn picture(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        let mut out = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                out.extend_from_slice(&f(x, y));
            }
        }
        out
    }

    /// A black disc on white paper — the shape of every line-art trace.
    fn disc(w: usize, h: usize, r: f64) -> Vec<u8> {
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        picture(w, h, |x, y| {
            let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
            if d < r {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        })
    }

    /// **The same picture traces the same way every time.** A tracer that
    /// reshuffled its palette between runs would make re-tracing after nudging
    /// a setting reshuffle every colour in the result.
    #[test]
    fn tracing_is_deterministic() {
        let px = disc(64, 64, 20.0);
        let o = TraceOptions::line_art();
        let a = trace(64, 64, &px, &o);
        let b = trace(64, 64, &px, &o);
        assert_eq!(a.shapes.len(), b.shapes.len());
        assert_eq!(a.message, b.message);
        for (x, y) in a.shapes.iter().zip(&b.shapes) {
            assert_eq!(x.path.elements().len(), y.path.elements().len());
        }
    }

    /// **Line art comes back as the ink alone.** With the paper dropped there
    /// is one shape — the disc — and nothing behind it, which is what makes it
    /// paintable.
    #[test]
    fn line_art_drops_the_paper() {
        let px = disc(64, 64, 20.0);
        let report = trace(64, 64, &px, &TraceOptions::line_art());
        assert_eq!(report.regions, 1, "{}", report.message);
        let shape = &report.shapes[0];
        let bb = shape.path.bounding_box();
        // The disc is 40 across in the middle of a 64 square.
        assert!(bb.width() > 30.0 && bb.width() < 50.0, "traced {bb:?}");
        assert!(bb.center().x > 25.0 && bb.center().x < 39.0, "off centre: {bb:?}");
    }

    /// **Keeping the paper keeps both.** The option is a choice, and a
    /// photograph needs its background.
    #[test]
    fn keeping_the_background_keeps_both_regions() {
        let px = disc(64, 64, 20.0);
        let report = trace(
            64,
            64,
            &px,
            &TraceOptions {
                drop_background: false,
                ..TraceOptions::line_art()
            },
        );
        assert_eq!(report.regions, 2, "{}", report.message);
        // Largest first: the paper is bigger than the disc.
        assert!(
            report.shapes[0].path.bounding_box().area()
                > report.shapes[1].path.bounding_box().area()
        );
    }

    /// **A hole stays a hole.** A ring traced as a solid disc is the classic
    /// tracer failure, and it is what the non-zero fill is there to prevent.
    #[test]
    fn a_ring_keeps_its_hole() {
        let (w, h) = (80usize, 80usize);
        let (cx, cy) = (40.0, 40.0);
        let px = picture(w, h, |x, y| {
            let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
            if (14.0..30.0).contains(&d) {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let report = trace(w as u32, h as u32, &px, &TraceOptions::line_art());
        assert_eq!(report.regions, 1, "{}", report.message);
        let shape = &report.shapes[0];
        assert_eq!(
            shape.fill.as_ref().map(|f| f.rule),
            Some(FillMode::NonZero),
            "a traced region must punch its holes rather than filling over them"
        );
        // Two loops: the outside and the hole.
        let moves = shape
            .path
            .elements()
            .iter()
            .filter(|e| matches!(e, buzz_geom::PathEl::MoveTo(_)))
            .count();
        assert_eq!(moves, 2, "the hole was lost");
    }

    /// **Specks are thrown away.** A quantised photograph produces thousands of
    /// one-pixel islands; left in, they make a document nothing can open.
    #[test]
    fn specks_are_dropped_and_counted() {
        let mut px = disc(64, 64, 20.0);
        // Scatter single black pixels across the paper.
        for (x, y) in [(2usize, 2usize), (60, 3), (5, 58), (58, 59)] {
            let i = (y * 64 + x) * 4;
            px[i..i + 3].copy_from_slice(&[0, 0, 0]);
        }
        let report = trace(64, 64, &px, &TraceOptions::line_art());
        assert_eq!(report.specks, 4, "{}", report.message);
        assert_eq!(report.regions, 1, "only the disc should survive");
    }

    /// **Detail trades points for fidelity, in that direction.** The knob has
    /// to do what it says or it is worse than no knob.
    #[test]
    fn more_detail_keeps_more_points() {
        let px = disc(96, 96, 36.0);
        let fine = trace(
            96,
            96,
            &px,
            &TraceOptions {
                detail: 0.4,
                smooth: 0.0,
                ..TraceOptions::line_art()
            },
        );
        let coarse = trace(
            96,
            96,
            &px,
            &TraceOptions {
                detail: 4.0,
                smooth: 0.0,
                ..TraceOptions::line_art()
            },
        );
        let points = |r: &TraceReport| r.shapes[0].path.elements().len();
        assert!(
            points(&fine) > points(&coarse),
            "fine kept {} points, coarse kept {}",
            points(&fine),
            points(&coarse)
        );
    }

    /// **Transparent pixels are not artwork.** A cut-out must arrive as a
    /// cut-out, not as a rectangle with the shape knocked out of it.
    #[test]
    fn transparency_is_not_traced() {
        let px = picture(40, 40, |x, y| {
            if (10..30).contains(&x) && (10..30).contains(&y) {
                [200, 30, 30, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let report = trace(
            40,
            40,
            &px,
            &TraceOptions {
                colours: 2,
                drop_background: false,
                ..TraceOptions::default()
            },
        );
        assert_eq!(report.regions, 1, "{}", report.message);
        let bb = report.shapes[0].path.bounding_box();
        assert!(bb.width() < 25.0 && bb.height() < 25.0, "traced the void too: {bb:?}");
    }

    /// **The palette is asked for, not assumed.** Two colours from a two-colour
    /// picture, and never more than were asked for.
    #[test]
    fn the_palette_is_the_size_requested() {
        let px = disc(64, 64, 20.0);
        assert_eq!(quantise(&px, 2).len(), 2);
        // A picture with only two colours in it cannot yield eight.
        assert!(quantise(&px, 8).len() <= 8);
    }

    /// **An empty or malformed picture says so** rather than panicking or
    /// returning a document full of nothing.
    #[test]
    fn nothing_in_produces_a_reason_out() {
        let report = trace(0, 0, &[], &TraceOptions::default());
        assert!(report.shapes.is_empty());
        assert!(!report.message.is_empty());

        let clear = picture(8, 8, |_, _| [0, 0, 0, 0]);
        let report = trace(8, 8, &clear, &TraceOptions::default());
        assert!(report.shapes.is_empty());
        assert!(report.message.contains("transparent"), "{}", report.message);
    }
}
