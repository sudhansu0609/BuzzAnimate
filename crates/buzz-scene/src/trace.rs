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
//! 1. **Quantise** the picture to a small palette ([`quantise`]), by k-means
//!    in CIELAB. Two colours is line art; a dozen is a poster.
//! 2. **Find the regions** of each palette colour, and merge the specks into
//!    the region around them.
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

use std::collections::HashMap;
use std::sync::Arc;

use crate::bucket::{Grid, simplify, trace_contours};
use crate::object::ShapeData;
use buzz_geom::{Affine, BezPath, FillMode, Point, Vec2};
use peniko::Color;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// What a trace looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TraceMode {
    /// Decide automatically between LineArt and ColourLines based on the image content.
    #[default]
    Auto,
    /// Regions of flat colour, outlined and filled — the original Trace
    /// Bitmap.
    Outline,
    /// Centreline strokes, for a scan of a drawing. See `trace_lines`.
    LineArt,
    /// Regions of flat colour plus equal-width outline strokes along contrast boundaries and silhouette.
    ColourLines,
    /// Both: the regions, then the lines over them.
    Both,
    /// Dark ink as smooth filled shapes: outlines only, the way vectorizer.ai
    /// traces a line drawing. See `trace_ink`.
    Ink,
    /// The ink as `Ink` traces it, over flat colour fills that run under the
    /// lines: a coloured cartoon traced the way vectorizer.ai does. See
    /// `trace_colour`.
    InkAndColour,
}

/// How a traced line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LineStyle {
    /// A plain stroked centreline, one width.
    Stroke,
    /// A filled, variable-width outline — the exact effect of the brush tool.
    /// The default, because that is what most people mean by "trace my line
    /// art".
    #[default]
    Brush,
}

/// **How the line tracer decides which pixels are ink.**
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InkDetect {
    /// One luma threshold for the whole picture: a pixel is ink when its
    /// 3x3-blurred luma (over white) is below it. `Some(t)` is `t`; `None` defers
    /// to [`TraceOptions::ink_threshold`], which is Otsu's method when that is
    /// `None` too. The default, and what Trace Bitmap uses.
    Global(Option<u8>),
    /// **Locally dark, thin ink.** A pixel is ink when it is opaque (alpha above
    /// 128) and its blurred luma is below `ratio` times the *background* — the
    /// 80th-percentile luma of the opaque pixels in the `window` x `window` box
    /// round it — and below `max_luma` outright. A dark garment's interior is not
    /// ink (its background is itself dark); its edge against skin or paper is a
    /// thin band, which is the stroke the drawing has there. Finds the 2-3 px
    /// dark-brown outlines of a face or hand that no single global threshold
    /// separates from dark cloth.
    LocalContrast {
        /// Ink is below this fraction of the background luma (0.62 is a good cartoon outline).
        ratio: f32,
        /// ... and below this luma (0-255) whatever the background.
        max_luma: u8,
        /// The side of the square box the background is measured over, in pixels.
        window: usize,
        /// ... and the pixel's CIELAB colour is within this ΔE of
        /// [`TraceOptions::line_colour`], so a saturated dark red/green/purple patch
        /// (a plaid line, a sari print) is a fill and not ink. `30.0` suits a
        /// character; `f64::INFINITY` switches the test off (so does a `line_colour`
        /// of `None`).
        max_delta_e_to_line: f64,
    },
}

impl Default for InkDetect {
    fn default() -> Self {
        Self::Global(None)
    }
}

/// How to trace.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
    /// **Regions, lines, or both.** See [`TraceMode`].
    pub mode: TraceMode,
    /// Minimum colour contrast (in CIELAB ΔE) between adjacent regions to draw
    /// an outline stroke between them in `ColourLines` mode. Default is 18.0.
    pub outline_contrast: f64,
    /// Override colour for outline strokes. `None` uses the darkest colour in
    /// the palette.
    pub line_colour: Option<Color>,
    /// How much a traced line is smoothed, `0` to `1`. See
    /// [`crate::trace_lines`].
    pub line_smooth: f64,
    /// **The shortest line worth keeping**, in pixels.
    pub min_line_length: f64,
    /// How a traced line is drawn. See [`LineStyle`].
    pub line_style: LineStyle,
    /// Trace the paper enclosed by lines as fills too, not just the ink.
    pub fill_enclosed: bool,
    /// **Beyond this radius, ink is a mass rather than a line**, in pixels —
    /// traced as a filled region instead of a centreline. Keeps a blob fused
    /// to a line from being skeletonised into a knot.
    pub max_line_radius: f64,
    /// The luma below which a pixel is ink. `None` finds it automatically, by
    /// Otsu's method. Used when [`TraceOptions::ink_detect`] is `Global(None)`
    /// (the default), so existing callers keep working.
    pub ink_threshold: Option<u8>,
    /// **How ink is told from paper.** See [`InkDetect`]; `Global(None)` (the
    /// default) is the single threshold of `ink_threshold`.
    pub ink_detect: InkDetect,
    /// **Trace masses as filled shapes** (the default). When `false`, ink fatter
    /// than `max_line_radius` is dropped from the ink output instead — only the
    /// thin part (strokes, outlines) is skeletonised and traced; a caller that
    /// fills those regions itself (the character pipeline) asks for this.
    pub ink_masses: bool,
    /// **Force every `Stroke`-style line to exactly this width**, in pixels,
    /// instead of the width measured off the ink. `None` measures as usual.
    /// For a caller that already knows the line weight it wants — the
    /// character pipeline tracing a clean, single-weight line drawing — and
    /// would rather have it exact than have it read off noisy scan pixels.
    /// Ignored for `LineStyle::Brush`, whose whole point is a measured,
    /// varying width.
    pub uniform_width: Option<f64>,
    /// **`TraceMode::Ink` only**: pixels added to each side of every ink
    /// shape. `0` (the default) keeps the traced edge exactly where the ink
    /// level puts it — the source's own line weight; `0.5` reads as that plus
    /// a touch. Negative values thin every line by that much per side, but
    /// never erode a shape too thin to survive it.
    pub ink_bold: f64,
    /// **The tracer's look knobs** (ink and colour stages). See
    /// [`crate::trace_tuning`]; the default is the tracer as it always was.
    pub tuning: crate::trace_tuning::TraceTuning,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            colours: 6,
            detail: 1.5,
            smooth: 0.5,
            speckle: 16,
            drop_background: false,
            mode: TraceMode::Auto,
            outline_contrast: 18.0,
            line_colour: None,
            line_smooth: 0.5,
            min_line_length: 8.0,
            line_style: LineStyle::Brush,
            fill_enclosed: false,
            max_line_radius: 8.0,
            ink_threshold: None,
            ink_detect: InkDetect::default(),
            ink_masses: true,
            uniform_width: None,
            ink_bold: 0.0,
            tuning: crate::trace_tuning::TraceTuning::default(),
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
            mode: TraceMode::LineArt,
            line_style: LineStyle::Brush,
            fill_enclosed: true,
            ..Self::default()
        }
    }

    /// **Ink only, as smooth filled shapes**: the way vectorizer.ai traces a
    /// line drawing. See `trace_ink`.
    pub fn ink() -> Self {
        Self {
            mode: TraceMode::Ink,
            speckle: 12,
            line_smooth: 0.5,
            ink_bold: 0.0,
            drop_background: true,
            ..Self::default()
        }
    }

    /// **Ink as smooth filled shapes over flat colour fills**: the ink of
    /// [`TraceOptions::ink`] on top of up to 24 colour regions that run under
    /// the lines, the paper dropped. See `trace_colour`.
    pub fn ink_colour() -> Self {
        Self {
            mode: TraceMode::InkAndColour,
            colours: 24,
            speckle: 12,
            line_smooth: 0.5,
            ink_bold: 0.0,
            drop_background: true,
            ..Self::default()
        }
    }

    /// These options with the user's saved tuning
    /// ([`crate::trace_tuning::TraceTuning::load_or_default`]: the app's
    /// `trace_tuning.json`, or `BUZZ_TRACE_TUNING`) in place of
    /// [`TraceOptions::tuning`]; and `BUZZ_TRACE_BOLD=<px>`, when set to a
    /// number, in place of [`TraceOptions::ink_bold`]. The presets above stay
    /// pure — this reads a file and the environment — so a caller acting for
    /// the user asks for it explicitly.
    pub fn with_saved_tuning(self) -> Self {
        let ink_bold = std::env::var("BUZZ_TRACE_BOLD")
            .ok()
            .and_then(|bold| bold.trim().parse::<f64>().ok())
            .unwrap_or(self.ink_bold);
        Self { tuning: crate::trace_tuning::TraceTuning::load_or_default(), ink_bold, ..self }
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
    /// Regions smaller than `speckle`: merged into a neighbouring region, or
    /// thrown away when nothing but transparent / background touches them.
    pub specks: usize,
    /// Centreline strokes traced (`TraceMode::LineArt` or `Both`).
    pub strokes: usize,
    /// Enclosed-paper fills traced alongside the lines.
    pub fills: usize,
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
///
/// Dispatches on [`TraceOptions::mode`]: `Outline` is the original region
/// trace, unchanged; `LineArt` is centreline strokes (see
/// [`crate::trace_lines`]); `Both` runs the regions first and the lines over
/// them, in the order that would have been painted.
pub fn trace(width: u32, height: u32, pixels: &[u8], options: &TraceOptions) -> TraceReport {
    match options.mode {
        TraceMode::Auto => {
            let is_ink = crate::trace_lines::is_ink_drawing(
                width as usize,
                height as usize,
                pixels,
                options,
            );
            if is_ink {
                let mut opts = *options;
                opts.mode = TraceMode::LineArt;
                crate::trace_lines::trace_lines(width, height, pixels, &opts)
            } else {
                let mut opts = *options;
                opts.mode = TraceMode::ColourLines;
                trace_regions_upsampled(width, height, pixels, &opts)
            }
        }
        TraceMode::Outline | TraceMode::ColourLines => {
            trace_regions_upsampled(width, height, pixels, options)
        }
        TraceMode::LineArt => crate::trace_lines::trace_lines(width, height, pixels, options),
        TraceMode::Both => {
            let mut report = trace_regions(width, height, pixels, options, 1.0);
            let lines = crate::trace_lines::trace_lines(width, height, pixels, options);
            report.message = if lines.strokes > 0 || lines.fills > 0 {
                format!("{} \u{2014} {}", report.message, lines.message)
            } else {
                report.message
            };
            report.shapes.extend(lines.shapes);
            report.specks += lines.specks;
            report.strokes += lines.strokes;
            report.fills += lines.fills;
            report
        }
        TraceMode::Ink => crate::trace_ink::trace_ink(width, height, pixels, options),
        TraceMode::InkAndColour => crate::trace_colour::trace_ink_and_colour(width, height, pixels, options),
    }
}

/// **The ink coverage field** `trace_ink` thresholds and traces: `F` per
/// native pixel, `0` (paper) to `1` (solid ink) — the thin dark line against
/// whatever it is drawn on, and the dark masses (hair, pupils, shoes),
/// without blur. For the character pipeline, which wants the same field
/// `TraceMode::Ink` sees without running the rest of the trace.
pub fn ink_field(width: u32, height: u32, pixels: &[u8]) -> Vec<f32> {
    crate::trace_ink::ink_coverage(width, height, pixels, &crate::trace_tuning::InkTuning::default())
}

/// **The ink mask the line tracer works from**: `pixels` (straight RGBA8)
/// composited over white, blurred, thresholded (`threshold`, or Otsu's method
/// for `None`) and cleaned of ink specks under `speckle` pixels and pinholes —
/// exactly the mask `TraceMode::LineArt` traces. `true` is ink. For a caller
/// that must see the same ink the tracer will (the character pipeline paints
/// fills *under* it).
pub fn ink_mask(width: u32, height: u32, pixels: &[u8], threshold: Option<u8>, speckle: usize) -> Vec<bool> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return vec![false; w * h];
    }
    let mut ink = crate::trace_lines::ink_mask(w, h, pixels, threshold);
    crate::trace_lines::clean(&mut ink, w, h, speckle.max(1));
    ink
}

/// The luma threshold [`ink_mask`] uses when given `None` (Otsu's method over
/// the blurred picture composited over white). `128` for an empty picture.
pub fn ink_threshold(width: u32, height: u32, pixels: &[u8]) -> u8 {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return 128;
    }
    crate::trace_lines::auto_ink_threshold(w, h, pixels)
}

/// **The ink split into lines and masses** (T9e): see [`ink_components`].
#[derive(Debug, Clone, PartialEq)]
pub struct InkComponents {
    /// The ink of every LINE component (`width * height`, `true` is ink): what the line
    /// tracer skeletonises with `ink_masses: false`. Nothing fatter than
    /// `options.max_line_radius` is in it.
    pub line_mask: Vec<bool>,
    /// The rest of the detected ink: the MASS components (a brow, a pupil, hair, a shoe)
    /// and the fat cores cut out of the lines.
    pub mass_mask: Vec<bool>,
    /// The typical pen width in px (source resolution), the median over components of
    /// twice their median distance-transform radius, clamped to [1.5, 6].
    pub line_width_typical: f64,
}

/// **Classify the ink of `pixels` as line or mass by compactness** (T9e). The ink is
/// the mask `options.ink_detect` chooses, cleaned of specks under `options.speckle`; each
/// 8-connected component of it has `ratio = area / skeleton length` (a component with
/// no skeleton: `area / max(bbox side)`), and is a MASS iff `ratio > 2.5 x
/// line_width_typical`, else a line — even where a pressed stroke thickens locally, so
/// an outline stays one continuous stroke. `max_line_radius` stays only as the upper
/// bound on a line's thickness. This is exactly the split `TraceMode::LineArt` makes
/// with `ink_masses: false` (it traces `line_mask` only).
pub fn ink_components(width: u32, height: u32, pixels: &[u8], options: &TraceOptions) -> InkComponents {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return InkComponents {
            line_mask: vec![false; w * h],
            mass_mask: vec![false; w * h],
            line_width_typical: 1.5,
        };
    }
    let c = crate::trace_lines::classify_ink(w, h, pixels, options);
    InkComponents { line_mask: c.line, mass_mask: c.mass, line_width_typical: c.line_width_typical }
}

/// **The ink the line tracer actually skeletonises** for `options`: the mask
/// chosen by `options.ink_detect` ([`InkDetect`]), cleaned of specks under
/// `options.speckle`, and — when `options.ink_masses` is `false` — only its LINE
/// components (T9e: masses by compactness, see [`ink_components`]).
/// `true` is ink. Exactly the mask `TraceMode::LineArt` traces under those
/// options, for a caller that must see the same ink (the character pipeline
/// paints its fills *under* it; `trace_metrics::compute_ink_coverage` measures
/// against it).
pub fn thin_ink_mask(width: u32, height: u32, pixels: &[u8], options: &TraceOptions) -> Vec<bool> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return vec![false; w * h];
    }
    crate::trace_lines::traced_ink(w, h, pixels, options)
}

/// **Small pictures are upscaled before tracing.**
///
/// A character export at a few hundred pixels does not have enough
/// resolution for a clean eye or fingertip — the region flood is working one
/// blocky sample per feature. Bicubic-upscaling to at least 1024 px on the
/// long edge first gives the tracer the resolution to keep small, salient
/// detail; the geometry is scaled back down afterward, so the caller — which
/// placed the picture and knows nothing about this internal upscale — still
/// gets paths sized to the picture it handed in.
fn trace_regions_upsampled(
    width: u32,
    height: u32,
    pixels: &[u8],
    options: &TraceOptions,
) -> TraceReport {
    const TARGET_LONG_EDGE: u32 = 1024;
    let long_edge = width.max(height);
    if long_edge == 0 || long_edge >= TARGET_LONG_EDGE {
        return trace_regions(width, height, pixels, options, 1.0);
    }
    let factor = (TARGET_LONG_EDGE + long_edge - 1) / long_edge;
    if factor <= 1 {
        return trace_regions(width, height, pixels, options, 1.0);
    }

    // **Premultiplied, so the transparent surround does not bleed a dark
    // fringe into a cutout's edge.** The `image` crate resizes RGBA as four
    // independent channels; against a fully transparent (and here, black)
    // surround that would drag every partly-covered edge pixel's colour
    // toward black as its alpha fades, inventing a ring of colour that was
    // never in the picture. Premultiplying first makes a transparent pixel's
    // colour irrelevant to its neighbours, the way every compositor expects;
    // unpremultiplying after hands `trace_regions` the straight-alpha colours
    // it already assumes everywhere else.
    let mut premultiplied = pixels.to_vec();
    for px in premultiplied.chunks_exact_mut(4) {
        let a = px[3] as u16;
        px[0] = ((px[0] as u16 * a) / 255) as u8;
        px[1] = ((px[1] as u16 * a) / 255) as u8;
        px[2] = ((px[2] as u16 * a) / 255) as u8;
    }
    let Some(image) = ::image::RgbaImage::from_raw(width, height, premultiplied) else {
        return trace_regions(width, height, pixels, options, 1.0);
    };
    let (up_w, up_h) = (width * factor, height * factor);
    let mut upscaled = ::image::imageops::resize(
        &image,
        up_w,
        up_h,
        ::image::imageops::FilterType::CatmullRom,
    )
    .into_raw();
    for px in upscaled.chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 {
            continue;
        }
        px[0] = ((px[0] as u32 * 255) / a as u32).min(255) as u8;
        px[1] = ((px[1] as u32 * 255) / a as u32).min(255) as u8;
        px[2] = ((px[2] as u32 * 255) / a as u32).min(255) as u8;
    }

    let mut report = trace_regions(up_w, up_h, &upscaled, options, factor as f64);

    let down = Affine::scale(1.0 / factor as f64);
    for shape in &mut report.shapes {
        shape.path = down * shape.path.clone();
        if let Some(ref mut stroke) = shape.stroke {
            stroke.width /= factor as f64;
        }
    }
    if report.regions > 0 || report.specks > 0 {
        report.message = format!("{} (upscaled {factor}\u{00d7} for detail)", report.message);
    }
    report
}

/// The original Trace Bitmap: regions of flat colour, outlined and filled.
///
/// `factor` is how much larger `width`×`height` is than the picture the
/// caller actually handed in — `1.0` when [`trace_regions_upsampled`] did no
/// upscaling, or `TraceMode::Both` called straight in. Every length that is
/// meant to mean something in the *picture's* own pixels — the speck size,
/// the simplify tolerance, the corner window — is scaled up by it, so that
/// tracing at extra internal resolution changes how much *is seen*, not what
/// counts as a speck or a corner.
fn trace_regions(
    width: u32,
    height: u32,
    pixels: &[u8],
    options: &TraceOptions,
    factor: f64,
) -> TraceReport {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return TraceReport {
            message: "There is nothing in that picture to trace".into(),
            ..Default::default()
        };
    }

    let colours = options.colours.clamp(2, 64);
    let palette = quantise_scaled(pixels, w, h, colours, factor);
    if palette.is_empty() {
        return TraceReport {
            message: "That picture is entirely transparent".into(),
            ..Default::default()
        };
    }
    let palette_lab: Vec<[f64; 3]> = palette.iter().map(|&c| srgb_to_lab(c)).collect();

    // Every pixel's own CIELAB colour, and its nearest palette entry — or
    // `None` where it is transparent.
    let mut lab: Vec<[f64; 3]> = Vec::with_capacity(w * h);
    let mut index: Vec<Option<u8>> = Vec::with_capacity(w * h);
    for i in 0..w * h {
        let px = &pixels[i * 4..i * 4 + 4];
        if px[3] < 128 {
            lab.push([0.0; 3]);
            index.push(None);
            continue;
        }
        let c = srgb_to_lab([px[0], px[1], px[2]]);
        lab.push(c);
        index.push(Some(nearest_lab(&palette_lab, c) as u8));
    }

    // **Anti-aliased pixels join a neighbour, not a speck.** A pixel whose
    // colour sits near the blend line between two palette colours present
    // right around it is an edge's antialiasing, not a colour of its own;
    // reassigning it to the nearer of the two keeps the speck rule for
    // genuinely isolated regions, instead of the fringe of one-pixel dither
    // a soft edge leaves everywhere it turns.
    reassign_blended_pixels(&mut index, &lab, &palette_lab, w, h);

    let mut counts = vec![0usize; palette.len()];
    for idx in index.iter().flatten() {
        counts[*idx as usize] += 1;
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

    // **The default speck size scales with the picture.** A fixed pixel count
    // is the right size for nothing: sixteen pixels is a fleck on a large
    // photograph and half an eye on a small character sheet. An explicit
    // request overrides it, the same as it always could.
    let speckle = if options.speckle == TraceOptions::default().speckle {
        // The formula is in the picture's own pixels; `factor²` converts
        // that back up into traced-grid pixels, the units `cells.len()`
        // below is actually counted in.
        let f2 = (factor.max(1.0) * factor.max(1.0)).max(1.0);
        let picture_area = (w as f64 * h as f64) / f2;
        (((picture_area / 200_000.0).max(4.0)) * f2).round() as usize
    } else {
        ((options.speckle.max(1) as f64) * factor * factor).round() as usize
    };

    let mut region_list: Vec<(u8, Vec<usize>)> = Vec::new();
    // Cell -> index of the kept region it belongs to in `region_list`
    // (`u32::MAX` for none), so a speck can find the regions around it.
    let mut region_of: Vec<u32> = vec![u32::MAX; w * h];
    // Regions under the speck size, held back to be merged rather than
    // dropped (a dropped speck leaves a hole the background shows through).
    let mut specks_found: Vec<(u8, Vec<usize>)> = Vec::new();
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
            let push = |nx: usize, ny: usize, stack: &mut Vec<usize>, seen: &mut Vec<bool>| {
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

        if cells.len() < speckle {
            specks_found.push((colour, std::mem::take(&mut cells)));
            continue;
        }
        let at_index = region_list.len() as u32;
        for &c in &cells {
            region_of[c] = at_index;
        }
        region_list.push((colour, std::mem::take(&mut cells)));
    }

    // **A speck joins the region around it.** Each under-size region goes to
    // the kept region that owns most of its 4-neighbour cells; one touching
    // only transparent / background cells is dropped.
    //
    // Each kept region's own flooded cell count is remembered first: the
    // merged cells are appended after it, and the region's colour comes from
    // its own cells only. Each merged speck is also recorded (host region,
    // cells) so a contrasting one can be put back on top as its own shape.
    let own_lens: Vec<usize> = region_list.iter().map(|(_, cells)| cells.len()).collect();
    let mut merged_specks: Vec<(u32, Vec<usize>)> = Vec::new();
    let (merged, dropped) = merge_specks(
        &mut region_list,
        &mut region_of,
        specks_found,
        w,
        h,
        &mut merged_specks,
    );
    let specks = merged + dropped;

    // **Each region's outline, colour and fill, built independently of every
    // other region.** The flood above has to be one sequential scan over
    // shared state; once a region's own pixels are known, tracing its
    // boundary touches nothing that belongs to any other region, which is
    // exactly the shape of work rayon is for.
    let built: Vec<(usize, usize, ShapeData, [u8; 3])> = region_list
        .par_iter()
        .enumerate()
        .filter_map(|(i, (_, cells))| {
            build_region_shape(cells, own_lens[i], pixels, w, options, factor)
                .map(|(n, shape, rgb)| (i, n, shape, rgb))
        })
        .collect();
    let mut host_rgb: Vec<Option<[u8; 3]>> = vec![None; region_list.len()];
    let mut regions: Vec<(usize, ShapeData)> = Vec::with_capacity(built.len());
    for (i, n, shape, rgb) in built {
        host_rgb[i] = Some(rgb);
        regions.push((n, shape));
    }

    // **Largest first**, so the picture assembles back to front the way it
    // would have been painted: the big flats down, the detail over them.
    regions.sort_by(|a, b| b.0.cmp(&a.0));

    // **A merged speck that contrasts with its host is kept as its own
    // shape**, drawn after (above) every host, so a pupil's catchlight, a
    // button or a pen mark survives while the hole behind it stays filled.
    // Only a speck of at least 6 picture pixels whose own mean colour is more
    // than 20 ΔE from the host's fill counts; anything smaller or closer in
    // colour merges silently.
    let detail_min = (6.0 * factor * factor).round() as usize;
    let mut detail: Vec<(usize, ShapeData)> = merged_specks
        .par_iter()
        .filter_map(|(host, cells)| {
            if cells.len() < detail_min {
                return None;
            }
            let host = host_rgb[*host as usize]?;
            let own = mean_rgb(cells, pixels);
            if delta_e(srgb_to_lab(own), srgb_to_lab(host)) <= 20.0 {
                return None;
            }
            build_region_shape(cells, cells.len(), pixels, w, options, factor)
                .map(|(n, shape, _)| (n, shape))
        })
        .collect();
    detail.sort_by(|a, b| b.0.cmp(&a.0));
    let detail_count = detail.len();

    let kept = regions.len() + detail_count;
    let mut shapes: Vec<ShapeData> = regions.into_iter().map(|(_, s)| s).collect();
    shapes.extend(detail.into_iter().map(|(_, s)| s));
    let mut stroke_count = 0;

    if options.mode == TraceMode::ColourLines {
        let strokes = build_outline_strokes(
            &region_list,
            w,
            h,
            &palette,
            &palette_lab,
            options,
            factor,
        );
        stroke_count = strokes.len();
        shapes.extend(strokes);
    }

    let message = match (kept, specks) {
        (0, 0) => "Nothing in that picture came out as a shape".to_string(),
        (0, n) => {
            format!("Everything in that picture was smaller than the speck size ({n} regions)")
        }
        (k, _) => {
            let mut m = format!("Traced {k} shapes in {} colours", palette.len());
            if merged > 0 {
                m.push_str(&format!(", merged {merged} specks into their neighbours"));
            }
            if dropped > 0 {
                if merged > 0 {
                    m.push_str(&format!(", dropped {dropped}"));
                } else {
                    m.push_str(&format!(", dropped {dropped} specks"));
                }
            }
            if detail_count > 0 {
                m.push_str(&format!(", kept {detail_count} as detail"));
            }
            m
        }
    };
    let message = if stroke_count > 0 {
        format!("{message} (+ {stroke_count} outline strokes)")
    } else {
        message
    };

    TraceReport {
        shapes,
        regions: kept,
        specks,
        strokes: stroke_count,
        fills: 0,
        message,
    }
}

/// **Merge each speck into the kept region around it**, instead of dropping it
/// and leaving a hole the background shows through.
///
/// `specks` are the under-size regions; `regions` the kept ones, and
/// `region_of` maps each cell to its kept region's index (`u32::MAX` for
/// none). Each speck's cells are appended to the kept region that owns the
/// most of its 4-neighbour cells (ties go to the lower index). The pass
/// repeats until nothing changes, so a speck touching only other specks
/// follows them once they have joined. What is left — touching only
/// transparent or background cells — is dropped.
///
/// Each merged speck's own cells are also pushed to `record` as
/// `(host region index, cells)`, so the caller can tell a host's own cells
/// (the first ones, as counted before the merge) from the merged ones and can
/// emit a contrasting speck as a shape of its own.
///
/// Returns `(merged, dropped)`.
fn merge_specks(
    regions: &mut [(u8, Vec<usize>)],
    region_of: &mut [u32],
    specks: Vec<(u8, Vec<usize>)>,
    w: usize,
    h: usize,
    record: &mut Vec<(u32, Vec<usize>)>,
) -> (usize, usize) {
    let mut pending: Vec<Vec<usize>> = specks.into_iter().map(|(_, cells)| cells).collect();
    let mut merged = 0usize;
    loop {
        let mut changed = false;
        let mut still: Vec<Vec<usize>> = Vec::new();
        for cells in pending {
            // How many neighbour cells each kept region contributes.
            let mut votes: std::collections::BTreeMap<u32, usize> =
                std::collections::BTreeMap::new();
            for &at in &cells {
                let (x, y) = (at % w, at / w);
                let mut vote = |n: usize| {
                    let r = region_of[n];
                    if r != u32::MAX {
                        *votes.entry(r).or_insert(0) += 1;
                    }
                };
                if x > 0 {
                    vote(at - 1);
                }
                if x + 1 < w {
                    vote(at + 1);
                }
                if y > 0 {
                    vote(at - w);
                }
                if y + 1 < h {
                    vote(at + w);
                }
            }
            // Highest count; the map iterates in index order, so a strict
            // `>` leaves ties with the lower index.
            let mut best: Option<(u32, usize)> = None;
            for (&r, &n) in &votes {
                if best.map_or(true, |(_, bn)| n > bn) {
                    best = Some((r, n));
                }
            }
            match best {
                Some((r, _)) => {
                    for &c in &cells {
                        region_of[c] = r;
                    }
                    regions[r as usize].1.extend(cells.iter().copied());
                    record.push((r, cells));
                    merged += 1;
                    changed = true;
                }
                None => still.push(cells),
            }
        }
        pending = still;
        if !changed || pending.is_empty() {
            break;
        }
    }
    (merged, pending.len())
}

/// One region's outline, colour and fill — traced, corner-aware smoothed,
/// sealed against its neighbours, and coloured from its own pixels.
///
/// `cells` are linear pixel indices into the full `w`-wide picture; the grid
/// built here is sized to the region's own bounding box rather than the
/// whole picture, because regions now run in parallel and cannot share one
/// scratch grid the way the old sequential loop did.
///
/// The outline covers every cell, but the fill colour is the mean of the first
/// `own_len` cells only — a region's own flooded cells, not the specks merged
/// onto the end of the list. Returns the cell count, the shape and its fill
/// colour.
fn build_region_shape(
    cells: &[usize],
    own_len: usize,
    pixels: &[u8],
    w: usize,
    options: &TraceOptions,
    factor: f64,
) -> Option<(usize, ShapeData, [u8; 3])> {
    let (mut x0, mut y0) = (usize::MAX, usize::MAX);
    let (mut x1, mut y1) = (0usize, 0usize);
    for &at in cells {
        let (x, y) = (at % w, at / w);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut grid = Grid::new(bw, bh);
    for &at in cells {
        let (x, y) = (at % w, at / w);
        grid.set(x - x0, y - y0);
    }

    let path = outline_colour(&grid, options, factor)?;
    // Translate back out of the region's own local grid into picture space.
    let path = Affine::translate((x0 as f64, y0 as f64)) * path;
    // let path = expand_fill(&path, 0.5 * factor, BooleanOptions::default());

    let own = &cells[..own_len.clamp(1, cells.len())];
    let colour_rgb = region_fill_colour(own, pixels, w, &grid, x0, y0);
    let mut shape = ShapeData::filled(
        path,
        Color::from_rgb8(colour_rgb[0], colour_rgb[1], colour_rgb[2]),
    );
    // **Non-zero, because the tracer winds holes the other way.** The outer
    // loops and the interior loops come back with opposite directions, which
    // is exactly what non-zero reads as "solid, with that punched out of it".
    if let Some(fill) = shape.fill.as_mut() {
        fill.rule = FillMode::NonZero;
    }
    Some((cells.len(), shape, colour_rgb))
}

/// The plain mean colour of `cells` (every cell, no erosion).
fn mean_rgb(cells: &[usize], pixels: &[u8]) -> [u8; 3] {
    let mut sum = [0u64; 3];
    for &at in cells {
        let px = &pixels[at * 4..at * 4 + 4];
        sum[0] += px[0] as u64;
        sum[1] += px[1] as u64;
        sum[2] += px[2] as u64;
    }
    let n = (cells.len() as u64).max(1);
    [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
}

/// **A region's fill is the mean of its own interior pixels**, eroded by one
/// pixel to leave out the antialiasing blend at its own edge — the source's
/// actual colour, not the palette centre it happened to cluster nearest to.
///
/// `grid` is the region's own local grid (see [`build_region_shape`]), which
/// makes the erosion test a cheap bit lookup instead of a second pixel set.
fn region_fill_colour(
    cells: &[usize],
    pixels: &[u8],
    w: usize,
    grid: &Grid,
    x0: usize,
    y0: usize,
) -> [u8; 3] {
    let interior: Vec<usize> = cells
        .iter()
        .copied()
        .filter(|&at| {
            let (x, y) = (at % w, at / w);
            let (lx, ly) = (x - x0, y - y0);
            lx > 0
                && ly > 0
                && lx + 1 < grid.w
                && ly + 1 < grid.h
                && grid.get(lx - 1, ly)
                && grid.get(lx + 1, ly)
                && grid.get(lx, ly - 1)
                && grid.get(lx, ly + 1)
        })
        .collect();
    let source = if interior.is_empty() { cells } else { &interior };

    let mut sum = [0u64; 3];
    for &at in source {
        let px = &pixels[at * 4..at * 4 + 4];
        sum[0] += px[0] as u64;
        sum[1] += px[1] as u64;
        sum[2] += px[2] as u64;
    }
    let n = source.len() as u64;
    [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
}

/// **A blended pixel joins whichever of its neighbourhood's two colours it is
/// nearer to**, rather than keeping its own nearest-palette guess.
///
/// Anti-aliasing along a real edge leaves a band of colours that are not in
/// the palette at all — blends of the two colours either side. Classified on
/// their own they scatter into whichever palette entry they happen to land
/// nearest, which is where a great many one-pixel specks come from. A pixel
/// that lies close to the straight-line blend between two colours actually
/// present in its own 3×3 neighbourhood is almost certainly one of those
/// blends, not a colour of its own — so it is moved to whichever end of that
/// blend it is closer to.
fn reassign_blended_pixels(
    index: &mut [Option<u8>],
    lab: &[[f64; 3]],
    palette_lab: &[[f64; 3]],
    w: usize,
    h: usize,
) {
    let original = index.to_vec();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if original[i].is_none() {
                continue;
            }

            let mut neighbours: Vec<u8> = Vec::with_capacity(9);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                        continue;
                    }
                    if let Some(c) = original[ny as usize * w + nx as usize] {
                        if !neighbours.contains(&c) {
                            neighbours.push(c);
                        }
                    }
                }
            }
            if neighbours.len() < 2 {
                continue;
            }

            let mut best: Option<(f64, u8, u8)> = None;
            for a in 0..neighbours.len() {
                for b in (a + 1)..neighbours.len() {
                    let (ia, ib) = (neighbours[a], neighbours[b]);
                    let d = point_to_segment(
                        lab[i],
                        palette_lab[ia as usize],
                        palette_lab[ib as usize],
                    );
                    if d < 6.0 && best.map_or(true, |(bd, _, _)| d < bd) {
                        best = Some((d, ia, ib));
                    }
                }
            }
            if let Some((_, ia, ib)) = best {
                let da = delta_e(lab[i], palette_lab[ia as usize]);
                let db = delta_e(lab[i], palette_lab[ib as usize]);
                index[i] = Some(if da <= db { ia } else { ib });
            }
        }
    }
}

/// Distance from `p` to the segment `a`–`b`, clamped to the segment rather
/// than the infinite line through it — a colour far past one end of the
/// blend is not a blend, however collinear it happens to be with it.
fn point_to_segment(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    if len2 < 1e-9 {
        return delta_e(p, a);
    }
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let t = ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2).clamp(0.0, 1.0);
    let closest = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
    delta_e(p, closest)
}

/// One region's outline, traced, simplified and smoothed.
///
/// `None` when the region had no loop long enough to be a shape — a single
/// pixel, or a one-pixel-wide thread.
pub(crate) fn outline(grid: &Grid, options: &TraceOptions) -> Option<BezPath> {
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

/// One traced region's outline, for the colour path: corners are found and
/// kept sharp, and everything between them is fit with a curve through the
/// simplified points.
///
/// This is deliberately a separate function from [`outline`] rather than a
/// branch inside it: [`outline`] is also how `trace_lines` builds its
/// enclosed-paper fills, and that behaviour is not part of this change.
///
/// `None` when the region had no loop long enough to be a shape.
/// One traced region's outline, for the colour path: corners are found and
/// kept sharp, and everything between them is fit with a curve through the
/// simplified points.
///
/// This is deliberately a separate function from [`outline`] rather than a
/// branch inside it: [`outline`] is also how `trace_lines` builds its
/// enclosed-paper fills, and that behaviour is not part of this change.
///
/// `None` when the region had no loop long enough to be a shape.
fn outline_colour(grid: &Grid, options: &TraceOptions, factor: f64) -> Option<BezPath> {
    let loops = trace_contours(grid);
    if loops.is_empty() {
        return None;
    }
    // Fixed in the traced-resolution pixel grid and scaled by `detail` and
    // `factor`, so that upscaling for detail refines the outline rather than
    // clinging to each subpixel staircase step.
    let tol = (0.4 * options.detail.max(0.0) * factor).max(0.05);
    let smooth = options.smooth.clamp(0.0, 1.0);

    let mut path = BezPath::new();
    for points in loops {
        let loop_path = if smooth <= 0.0 {
            polygon_path(&simplify(&points, tol))
        } else {
            smoothed_loop(&points, tol, smooth, factor)
        };
        if let Some(loop_path) = loop_path {
            for el in loop_path.elements() {
                path.push(*el);
            }
        }
    }
    (!path.elements().is_empty()).then_some(path)
}

/// A closed polygon straight through `points`, with no curve fitting.
fn polygon_path(points: &[(usize, usize)]) -> Option<BezPath> {
    if points.len() < 3 {
        return None;
    }
    let mut path = BezPath::new();
    path.move_to(Point::new(points[0].0 as f64, points[0].1 as f64));
    for &(x, y) in &points[1..] {
        path.line_to(Point::new(x as f64, y as f64));
    }
    path.close_path();
    Some(path)
}

/// One closed loop of raw traced points, smoothed between its corners and
/// left sharp at them.
fn smoothed_loop(
    points: &[(usize, usize)],
    tol: f64,
    smooth: f64,
    factor: f64,
) -> Option<BezPath> {
    let pf: Vec<Point> = points
        .iter()
        .map(|&(x, y)| Point::new(x as f64, y as f64))
        .collect();
    let corners = detect_corners(&pf, factor);
    let n = pf.len();
    let corner_idx: Vec<usize> = (0..n).filter(|&i| corners[i]).collect();

    if corner_idx.len() < 2 {
        // No sharp turn anywhere on this loop — a circle, an ellipse, a
        // blob — so the whole thing is "between corners". The existing
        // whole-path smoother, built for exactly this, handles the closed
        // loop without the seam an open Catmull-Rom would leave at an
        // arbitrary start point.
        let poly = polygon_path(&simplify(points, tol))?;
        return Some(buzz_geom::smooth(&poly, smooth));
    }

    // Split at the corners into runs, each simplified and then fit with a
    // Catmull-Rom curve through its own points — sharp at both ends, because
    // each run's tangents are built only from its own points, and smooth in
    // between. `smooth` doubles as the curve's tension: `0` pulls it
    // straight between the kept points, `1` is the free construction — the
    // same dial the rest of tracing uses for "how rounded".
    let mut path = BezPath::new();
    let mut started = false;
    for w in 0..corner_idx.len() {
        let start = corner_idx[w];
        let end = corner_idx[(w + 1) % corner_idx.len()];
        let run = ring_slice(&pf, start, end);
        if run.len() < 2 {
            continue;
        }
        let run_grid: Vec<(usize, usize)> =
            run.iter().map(|p| (p.x as usize, p.y as usize)).collect();
        let kept_pts: Vec<Point> = simplify(&run_grid, tol)
            .iter()
            .map(|&(x, y)| Point::new(x as f64, y as f64))
            .collect();
        if kept_pts.len() < 2 {
            continue;
        }
        let curve = buzz_geom::catmull_rom_tense(&kept_pts, smooth);
        if !started {
            path.move_to(kept_pts[0]);
            started = true;
        }
        for el in curve.elements().iter().skip(1) {
            path.push(*el);
        }
    }
    if started {
        path.close_path();
    }
    (!path.elements().is_empty()).then_some(path)
}

/// The points from index `start` to `end` inclusive, walking forward around a
/// closed loop of length `pf.len()` — wrapping past the end back to the
/// start when the walk does not reach `end` directly.
fn ring_slice(pf: &[Point], start: usize, end: usize) -> Vec<Point> {
    let n = pf.len();
    let mut out = Vec::new();
    let mut i = start;
    loop {
        out.push(pf[i]);
        if i == end {
            break;
        }
        i = (i + 1) % n;
    }
    out
}

/// **Corners of a traced loop**: points where the direction turns more than
/// 50° over a roughly 3-pixel window either side. A real corner survives
/// being looked at from a few pixels back and a few pixels ahead; a single
/// jag in the pixel staircase does not.
fn detect_corners(pf: &[Point], factor: f64) -> Vec<bool> {
    let n = pf.len();
    if n < 4 {
        return vec![false; n];
    }
    let win = (3.0 * factor).max(2.0);
    let mut angles = vec![0.0f64; n];
    for (i, angle_i) in angles.iter_mut().enumerate() {
        let back = walk(pf, i, -win);
        let fwd = walk(pf, i, win);
        let v1 = pf[i] - back;
        let v2 = fwd - pf[i];
        if v1.hypot() < 1e-6 || v2.hypot() < 1e-6 {
            continue;
        }
        let cos = (v1.dot(v2) / (v1.hypot() * v2.hypot())).clamp(-1.0, 1.0);
        let deg = cos.acos().to_degrees();
        if deg > 50.0 {
            *angle_i = deg;
        }
    }

    let mut out = vec![false; n];
    for i in 0..n {
        if angles[i] <= 50.0 {
            continue;
        }
        let mut is_max = true;
        // Walk backward within distance win
        let mut at = i;
        let mut travelled = 0.0;
        while travelled < win {
            let prev = (at + n - 1) % n;
            travelled += (pf[prev] - pf[at]).hypot();
            at = prev;
            if travelled >= win || at == i {
                break;
            }
            if angles[at] > angles[i] || (angles[at] == angles[i] && at < i) {
                is_max = false;
                break;
            }
        }
        if !is_max {
            continue;
        }
        // Walk forward within distance win
        let mut at = i;
        let mut travelled = 0.0;
        while travelled < win {
            let next = (at + 1) % n;
            travelled += (pf[next] - pf[at]).hypot();
            at = next;
            if travelled >= win || at == i {
                break;
            }
            if angles[at] > angles[i] || (angles[at] == angles[i] && at < i) {
                is_max = false;
                break;
            }
        }
        if is_max {
            out[i] = true;
        }
    }
    out
}

/// The point roughly `dist` pixels along the loop from index `i` — negative
/// walks backward — found by walking the raw points and accumulating
/// distance. A marching-squares trace steps about 1 px between points, so
/// this is the plan's "3-pixel window".
fn walk(pf: &[Point], i: usize, dist: f64) -> Point {
    let n = pf.len();
    let mut at = i;
    let mut travelled = 0.0;
    let step: isize = if dist >= 0.0 { 1 } else { -1 };
    let target = dist.abs();
    loop {
        let next = ((at as isize + step).rem_euclid(n as isize)) as usize;
        travelled += (pf[next] - pf[at]).hypot();
        at = next;
        if travelled >= target || at == i {
            break;
        }
    }
    pf[at]
}

// ---------------------------------------------------------------------------
// ColourLines outline strokes (T2)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct OutlineWorkEdge {
    node_a: usize,
    node_b: usize,
    points: Vec<Point>,
}

fn build_outline_strokes(
    region_list: &[(u8, Vec<usize>)],
    w: usize,
    h: usize,
    palette: &[[u8; 3]],
    palette_lab: &[[f64; 3]],
    options: &TraceOptions,
    factor: f64,
) -> Vec<ShapeData> {
    if region_list.is_empty() || w == 0 || h == 0 {
        return Vec::new();
    }

    let mut pixel_region = vec![None; w * h];
    for &(colour, ref cells) in region_list {
        for &at in cells {
            pixel_region[at] = Some(colour);
        }
    }

    let is_outline = |a: Option<u8>, b: Option<u8>| -> bool {
        match (a, b) {
            (Some(_), None) | (None, Some(_)) => true,
            (Some(c1), Some(c2)) => {
                if c1 == c2 {
                    false
                } else {
                    delta_e(palette_lab[c1 as usize], palette_lab[c2 as usize])
                        > options.outline_contrast
                }
            }
            (None, None) => false,
        }
    };

    let v_id = |vx: usize, vy: usize| -> usize { vy * (w + 1) + vx };
    let v_coords = |id: usize| -> (usize, usize) { (id % (w + 1), id / (w + 1)) };
    let num_v = (w + 1) * (h + 1);

    let mut edges: Vec<(usize, usize)> = Vec::new();

    // Horizontal edges
    for y in 0..=h {
        for x in 0..w {
            let top = if y > 0 { pixel_region[(y - 1) * w + x] } else { None };
            let bottom = if y < h { pixel_region[y * w + x] } else { None };
            if is_outline(top, bottom) {
                let u = v_id(x, y);
                let v = v_id(x + 1, y);
                edges.push((u.min(v), u.max(v)));
            }
        }
    }

    // Vertical edges
    for y in 0..h {
        for x in 0..=w {
            let left = if x > 0 { pixel_region[y * w + (x - 1)] } else { None };
            let right = if x < w { pixel_region[y * w + x] } else { None };
            if is_outline(left, right) {
                let u = v_id(x, y);
                let v = v_id(x, y + 1);
                edges.push((u.min(v), u.max(v)));
            }
        }
    }

    if edges.is_empty() {
        return Vec::new();
    }

    edges.sort_unstable();
    edges.dedup();

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); num_v];
    let mut vertex_edges: Vec<Vec<usize>> = vec![Vec::new(); num_v];
    for (e_idx, &(u, v)) in edges.iter().enumerate() {
        adj[u].push(v);
        adj[v].push(u);
        vertex_edges[u].push(e_idx);
        vertex_edges[v].push(e_idx);
    }

    let mut edge_used = vec![false; edges.len()];
    let mut work_edges: Vec<OutlineWorkEdge> = Vec::new();

    // 1. Trace chains starting from nodes with degree != 2
    let mut special_nodes: Vec<usize> = (0..num_v)
        .filter(|&u| adj[u].len() != 2 && !adj[u].is_empty())
        .collect();
    special_nodes.sort_unstable();

    for &start_node in &special_nodes {
        for &e_idx in &vertex_edges[start_node] {
            if edge_used[e_idx] {
                continue;
            }
            edge_used[e_idx] = true;
            let (u, v) = edges[e_idx];
            let next_node = if u == start_node { v } else { u };
            let (sx, sy) = v_coords(start_node);
            let (nx, ny) = v_coords(next_node);
            let mut pts = vec![
                Point::new(sx as f64, sy as f64),
                Point::new(nx as f64, ny as f64),
            ];
            let mut curr = next_node;
            while adj[curr].len() == 2 && curr != start_node {
                let next_e = vertex_edges[curr].iter().copied().find(|&e| !edge_used[e]);
                let Some(next_e_idx) = next_e else { break };
                edge_used[next_e_idx] = true;
                let (eu, ev) = edges[next_e_idx];
                let next = if eu == curr { ev } else { eu };
                let (px, py) = v_coords(next);
                pts.push(Point::new(px as f64, py as f64));
                curr = next;
            }
            work_edges.push(OutlineWorkEdge {
                node_a: start_node,
                node_b: curr,
                points: pts,
            });
        }
    }

    // 2. Trace isolated loops (where every node in the loop has degree 2)
    for e_idx in 0..edges.len() {
        if edge_used[e_idx] {
            continue;
        }
        edge_used[e_idx] = true;
        let (start_node, next_node) = edges[e_idx];
        let (sx, sy) = v_coords(start_node);
        let (nx, ny) = v_coords(next_node);
        let mut pts = vec![
            Point::new(sx as f64, sy as f64),
            Point::new(nx as f64, ny as f64),
        ];
        let mut curr = next_node;
        while curr != start_node {
            let next_e = vertex_edges[curr].iter().copied().find(|&e| !edge_used[e]);
            let Some(next_e_idx) = next_e else { break };
            edge_used[next_e_idx] = true;
            let (eu, ev) = edges[next_e_idx];
            let next = if eu == curr { ev } else { eu };
            let (px, py) = v_coords(next);
            pts.push(Point::new(px as f64, py as f64));
            curr = next;
        }
        work_edges.push(OutlineWorkEdge {
            node_a: start_node,
            node_b: curr,
            points: pts,
        });
    }

    // 3. Step H: Continuation at junctions
    let mut junction_nodes: Vec<usize> = Vec::new();
    {
        let mut deg = HashMap::new();
        for we in &work_edges {
            *deg.entry(we.node_a).or_insert(0) += 1;
            *deg.entry(we.node_b).or_insert(0) += 1;
        }
        for (node, count) in deg {
            if count >= 3 {
                junction_nodes.push(node);
            }
        }
    }
    junction_nodes.sort_unstable();

    for node in junction_nodes {
        loop {
            let incident: Vec<usize> = (0..work_edges.len())
                .filter(|&i| work_edges[i].node_a == node || work_edges[i].node_b == node)
                .collect();
            if incident.len() < 2 {
                break;
            }

            let k_dist = (4.0 * factor).round().max(2.0) as usize;
            let tangent = |idx: usize| -> Vec2 {
                let we = &work_edges[idx];
                let from_a = we.node_a == node;
                let pts = &we.points;
                let k = k_dist.min(pts.len() - 1);
                let dir = if from_a {
                    pts[k] - pts[0]
                } else {
                    pts[pts.len() - 1 - k] - pts[pts.len() - 1]
                };
                let len = dir.hypot();
                if len > 1e-9 { dir / len } else { Vec2::new(1.0, 0.0) }
            };

            let tangents: Vec<Vec2> = incident.iter().map(|&i| tangent(i)).collect();
            let mut best: Option<(usize, usize, f64)> = None;
            for a in 0..incident.len() {
                for b in (a + 1)..incident.len() {
                    let dot = tangents[a].x * tangents[b].x + tangents[a].y * tangents[b].y;
                    if best.is_none_or(|(_, _, best_dot)| dot < best_dot) {
                        best = Some((a, b, dot));
                    }
                }
            }

            let Some((a, b, dot)) = best else { break };
            // Continue if straightest continuation has dot < -0.5 (> 120 degrees apart)
            if dot > -0.5 {
                break;
            }

            let ia = incident[a];
            let ib = incident[b];

            let ea = &work_edges[ia];
            let eb = &work_edges[ib];
            let mut pts_a = if ea.node_b == node {
                ea.points.clone()
            } else {
                let mut p = ea.points.clone();
                p.reverse();
                p
            };
            let pts_b = if eb.node_a == node {
                eb.points[1..].to_vec()
            } else {
                let mut p = eb.points.clone();
                p.reverse();
                p[1..].to_vec()
            };
            pts_a.extend(pts_b);

            let new_node_a = if ea.node_b == node { ea.node_a } else { ea.node_b };
            let new_node_b = if eb.node_a == node { eb.node_b } else { eb.node_a };

            let spliced = OutlineWorkEdge {
                node_a: new_node_a,
                node_b: new_node_b,
                points: pts_a,
            };

            let (lo, hi) = (ia.min(ib), ia.max(ib));
            work_edges.remove(hi);
            work_edges.remove(lo);
            work_edges.push(spliced);
        }
    }

    let stroke_color = options.line_colour.unwrap_or_else(|| {
        let mut darkest_idx = 0;
        let mut min_l = f64::INFINITY;
        for (i, lab) in palette_lab.iter().enumerate() {
            if lab[0] < min_l {
                min_l = lab[0];
                darkest_idx = i;
            }
        }
        let rgb = palette[darkest_idx];
        Color::from_rgb8(rgb[0], rgb[1], rgb[2])
    });

    let source_long_edge = (w as f64 / factor).max(h as f64 / factor);
    let auto_width = 1.5_f64.max(source_long_edge / 400.0);
    let stroke_width_source = options.uniform_width.unwrap_or(auto_width);
    let stroke_width = stroke_width_source * factor;
    let tol = (0.4 * factor * options.detail.max(0.1)).max(0.1);

    let mut shapes = Vec::new();
    for edge in work_edges {
        let pts = edge.points;
        if pts.len() < 2 {
            continue;
        }
        let total_len: f64 = pts.windows(2).map(|w| (w[1] - w[0]).hypot()).sum();
        if total_len < 1.0 * factor {
            continue;
        }

        let is_closed = edge.node_a == edge.node_b
            && (pts[0] - pts[pts.len() - 1]).hypot() < 1e-6
            && pts.len() >= 4;

        if is_closed {
            let loop_pts: Vec<(usize, usize)> = pts[..pts.len() - 1]
                .iter()
                .map(|p| (p.x.round() as usize, p.y.round() as usize))
                .collect();
            let path = smoothed_loop(&loop_pts, tol, options.smooth, factor)
                .or_else(|| polygon_path(&loop_pts));
            if let Some(p) = path {
                shapes.push(ShapeData::stroked(p, stroke_color, stroke_width));
            }
        } else {
            let simplified = simplify_points(&pts, tol);
            if simplified.len() < 2 {
                continue;
            }
            let corners = detect_open_corners(&simplified, factor);
            let mut corner_indices: Vec<usize> = (0..simplified.len())
                .filter(|&i| i == 0 || i == simplified.len() - 1 || corners[i])
                .collect();
            corner_indices.dedup();

            let mut path = BezPath::new();
            path.move_to(simplified[0]);
            for w in 0..corner_indices.len() - 1 {
                let start = corner_indices[w];
                let end = corner_indices[w + 1];
                let run = &simplified[start..=end];
                if run.len() == 2 {
                    path.line_to(run[1]);
                } else if run.len() > 2 {
                    let curve = buzz_geom::catmull_rom_tense(run, options.smooth);
                    for el in curve.elements().iter().skip(1) {
                        path.push(*el);
                    }
                }
            }
            shapes.push(ShapeData::stroked(path, stroke_color, stroke_width));
        }
    }

    shapes
}

/// Douglas-Peucker over `Point` coordinates.
fn simplify_points(points: &[Point], tol: f64) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    fn perpendicular(p: Point, a: Point, b: Point) -> f64 {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            return (p - a).hypot();
        }
        ((p.x - a.x) * dy - (p.y - a.y) * dx).abs() / len
    }
    fn recurse(points: &[Point], tol: f64, out: &mut Vec<Point>) {
        let (first, last) = (points[0], points[points.len() - 1]);
        let mut index = 0;
        let mut max = 0.0;
        for (i, &p) in points.iter().enumerate().take(points.len() - 1).skip(1) {
            let d = perpendicular(p, first, last);
            if d > max {
                max = d;
                index = i;
            }
        }
        if max > tol {
            recurse(&points[..=index], tol, out);
            out.pop();
            recurse(&points[index..], tol, out);
        } else {
            out.push(first);
            out.push(last);
        }
    }
    let mut out = Vec::new();
    recurse(points, tol, &mut out);
    out
}

fn detect_open_corners(pf: &[Point], factor: f64) -> Vec<bool> {
    let n = pf.len();
    if n < 4 {
        return vec![false; n];
    }
    let win = (3.0 * factor).max(2.0);
    let mut angles = vec![0.0f64; n];
    for i in 1..n - 1 {
        let back = walk_open(pf, i, -win);
        let fwd = walk_open(pf, i, win);
        let v1 = pf[i] - back;
        let v2 = fwd - pf[i];
        if v1.hypot() < 1e-6 || v2.hypot() < 1e-6 {
            continue;
        }
        let cos = (v1.dot(v2) / (v1.hypot() * v2.hypot())).clamp(-1.0, 1.0);
        let deg = cos.acos().to_degrees();
        if deg > 50.0 {
            angles[i] = deg;
        }
    }

    let mut out = vec![false; n];
    for i in 1..n - 1 {
        if angles[i] <= 50.0 {
            continue;
        }
        let mut is_max = true;
        let mut at = i;
        let mut travelled = 0.0;
        while at > 0 && travelled < win {
            let prev = at - 1;
            travelled += (pf[prev] - pf[at]).hypot();
            at = prev;
            if angles[at] > angles[i] || (angles[at] == angles[i] && at < i) {
                is_max = false;
                break;
            }
        }
        if !is_max {
            continue;
        }
        let mut at = i;
        let mut travelled = 0.0;
        while at + 1 < n && travelled < win {
            let next = at + 1;
            travelled += (pf[next] - pf[at]).hypot();
            at = next;
            if angles[at] > angles[i] || (angles[at] == angles[i] && at < i) {
                is_max = false;
                break;
            }
        }
        if is_max {
            out[i] = true;
        }
    }
    out
}

fn walk_open(pf: &[Point], i: usize, dist: f64) -> Point {
    let n = pf.len();
    if n == 0 {
        return Point::ORIGIN;
    }
    let mut at = i;
    let mut travelled = 0.0;
    let step: isize = if dist >= 0.0 { 1 } else { -1 };
    let target = dist.abs();
    loop {
        let next_idx = at as isize + step;
        if next_idx < 0 || next_idx >= n as isize {
            break;
        }
        let next = next_idx as usize;
        let seg_len = (pf[next] - pf[at]).hypot();
        if travelled + seg_len >= target && seg_len > 1e-6 {
            let t = (target - travelled) / seg_len;
            return pf[at] + (pf[next] - pf[at]) * t;
        }
        travelled += seg_len;
        at = next;
    }
    pf[at]
}

// ---------------------------------------------------------------------------
// Quantisation
// ---------------------------------------------------------------------------

/// **Reduce the picture to `want` colours, by k-means in CIELAB.**
///
/// # Why CIELAB k-means and not median-cut RGB
///
/// Median cut splits on raw RGB spread, which is not how far apart two
/// colours actually *look* — it can spend a box on a wide range of near-
/// identical greens while merging a small but glaring red into its
/// neighbour. Clustering in CIELAB, where Euclidean distance tracks
/// perceived difference, keeps colours that read as different apart and
/// colours that read as the same together, which is what "the red bag came
/// out brown" was.
///
/// # Why the same picture still traces the same way twice
///
/// K-means++'s seeding is driven by a fixed, non-random stream keyed off the
/// picture's own pixel count, not the system clock or thread scheduling: the
/// same picture seeds, clusters and traces the same way every run —
/// re-tracing after nudging a setting still only reshuffles what that
/// setting changed.
///
/// The picture is sampled rather than clustered whole — up to 200,000
/// pixels — weighted **away from edges**: a pixel sitting on a sharp
/// gradient is almost certainly an antialiasing blend rather than a real
/// colour, and downweighting it keeps the clusters from being dragged toward
/// mud. After clustering, any colour covering at least 30 px of the actual
/// picture that still reads as clearly different from every cluster found —
/// ΔE > 15 — becomes a centre of its own too, up to eight extra: that is
/// what keeps a small but deliberate colour, an eye or a highlight, from
/// disappearing into whatever happened to be nearest.
pub fn quantise(pixels: &[u8], width: usize, height: usize, want: usize) -> Vec<[u8; 3]> {
    quantise_scaled(pixels, width, height, want, 1.0)
}

pub fn quantise_scaled(
    pixels: &[u8],
    width: usize,
    height: usize,
    want: usize,
    factor: f64,
) -> Vec<[u8; 3]> {
    let want = want.max(1);
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return Vec::new();
    }
    let count = width * height;

    // Every opaque pixel's CIELAB colour, kept around for the sampling, the
    // clustering and the salient-colour pass below.
    let mut lab = vec![[0.0f64; 3]; count];
    let mut opaque = vec![false; count];
    for i in 0..count {
        let px = &pixels[i * 4..i * 4 + 4];
        if px[3] >= 128 {
            lab[i] = srgb_to_lab([px[0], px[1], px[2]]);
            opaque[i] = true;
        }
    }
    if !opaque.iter().any(|&o| o) {
        return Vec::new();
    }

    let luma: Vec<f64> = lab.iter().map(|c| c[0]).collect();
    let gradient = sobel_magnitude(&luma, width, height);
    // A pixel this far from flat is almost certainly a blend between two
    // real colours rather than one of its own.
    const EDGE_THRESHOLD: f64 = 12.0;

    const MAX_SAMPLES: usize = 200_000;
    let step = (count / MAX_SAMPLES).max(1);
    let mut samples: Vec<[f64; 3]> = Vec::new();
    let mut weights: Vec<f64> = Vec::new();
    for i in (0..count).step_by(step) {
        if !opaque[i] {
            continue;
        }
        samples.push(lab[i]);
        weights.push(if gradient[i] < EDGE_THRESHOLD { 1.0 } else { 0.1 });
    }
    if samples.is_empty() {
        return Vec::new();
    }

    let k = want.min(samples.len());
    let mut centres = kmeans_plus_plus_seed(&samples, &weights, k);
    for _ in 0..8 {
        let assignments: Vec<usize> = samples.iter().map(|s| nearest_lab(&centres, *s)).collect();
        let mut sums = vec![[0.0f64; 3]; centres.len()];
        let mut totals = vec![0.0f64; centres.len()];
        for i in 0..samples.len() {
            let a = assignments[i];
            let w = weights[i];
            sums[a][0] += samples[i][0] * w;
            sums[a][1] += samples[i][1] * w;
            sums[a][2] += samples[i][2] * w;
            totals[a] += w;
        }
        for c in 0..centres.len() {
            // An empty cluster keeps its old centre rather than becoming
            // NaN; the next iteration's assignment either repopulates it or
            // leaves it exactly where a duplicate seed put it.
            if totals[c] > 0.0 {
                centres[c] = [
                    sums[c][0] / totals[c],
                    sums[c][1] / totals[c],
                    sums[c][2] / totals[c],
                ];
            }
        }
    }

    // Merge centres that are perceptually indistinguishable (ΔE < 6.0).
    let mut unique_centres: Vec<[f64; 3]> = Vec::with_capacity(centres.len());
    for c in centres {
        if !unique_centres.iter().any(|u| delta_e(*u, c) < 6.0) {
            unique_centres.push(c);
        }
    }
    let mut centres = unique_centres;

    // **Small but salient colours survive.** A pupil or a highlight can be a
    // few dozen pixels against a much larger picture — too small a share of
    // any sample to pull a cluster of its own — but if it reads as clearly
    // different it still deserves a swatch, or an eye disappears into its
    // skin. Two-colour requests (line art, binary masks) stay strict to
    // what was asked.
    let max_extra = if want <= 2 { 0 } else { 8 };
    const SALIENT_MIN_PX: usize = 30;
    const SALIENT_DELTA_E: f64 = 15.0;
    let mut buckets: HashMap<[u8; 3], (usize, [f64; 3])> = HashMap::new();
    for i in 0..count {
        if !opaque[i] {
            continue;
        }
        let px = &pixels[i * 4..i * 4 + 4];
        // Coarse binning: near-identical colours share a bucket, so a solid
        // patch's own antialiased fringe is not counted as its own colour.
        let key = [px[0] & !0x07, px[1] & !0x07, px[2] & !0x07];
        let entry = buckets.entry(key).or_insert((0, [0.0; 3]));
        entry.0 += 1;
        entry.1[0] += lab[i][0];
        entry.1[1] += lab[i][1];
        entry.1[2] += lab[i][2];
    }
    let salient_min_px = ((SALIENT_MIN_PX as f64) * factor * factor).round() as usize;
    let mut candidates: Vec<(usize, [f64; 3])> = buckets
        .into_values()
        .filter(|(n, _)| *n >= salient_min_px)
        .map(|(n, sum)| (n, [sum[0] / n as f64, sum[1] / n as f64, sum[2] / n as f64]))
        .collect();
    // Largest first, and a stable order independent of the hash map's own —
    // a picture must quantise the same way every time it is traced.
    candidates.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1[0].total_cmp(&b.1[0]))
            .then_with(|| a.1[1].total_cmp(&b.1[1]))
            .then_with(|| a.1[2].total_cmp(&b.1[2]))
    });
    let mut extra = 0;
    for (_, colour) in candidates {
        if extra >= max_extra {
            break;
        }
        let far_from_every_centre =
            centres.iter().all(|c| delta_e(*c, colour) > SALIENT_DELTA_E);
        // **A colour that sits on the blend line between two existing
        // centres is not a colour of its own.** A soft edge that has been
        // upscaled spreads its antialiasing over enough pixels to look like
        // a real, sizeable patch on its own — a grey band between black and
        // white can cover thousands of pixels once the picture has been
        // upscaled — but every one of those bands is just a point on the
        // straight line between the two colours either side of it, which a
        // genuinely distinct colour, off that line, is not.
        let on_a_blend_line = centres.iter().enumerate().any(|(i, &a)| {
            centres[i + 1..]
                .iter()
                .any(|&b| point_to_segment(colour, a, b) < SALIENT_DELTA_E)
        });
        if far_from_every_centre && !on_a_blend_line {
            centres.push(colour);
            extra += 1;
        }
    }

    let mut palette: Vec<[u8; 3]> = centres.into_iter().map(lab_to_srgb).collect();
    // A stable order, so the same picture traces to the same document.
    palette.sort();
    palette.dedup();
    palette
}

/// k-means++ seeding: the first centre is a weighted pick off the sample
/// stream, and each next centre is chosen with probability proportional to
/// its squared distance from the nearest centre so far — the standard
/// construction that keeps k-means from collapsing onto one cluster over the
/// picture's single biggest flat colour.
///
/// The "randomness" is a fixed, deterministic stream keyed off the sample
/// count, not the system's: the same picture must seed, cluster and trace
/// the same way every time.
fn kmeans_plus_plus_seed(samples: &[[f64; 3]], weights: &[f64], k: usize) -> Vec<[f64; 3]> {
    let mut rng = DeterministicRng::new(0x9E3779B97F4A7C15 ^ samples.len() as u64);
    let mut centres: Vec<[f64; 3]> = Vec::with_capacity(k);
    if samples.is_empty() || k == 0 {
        return centres;
    }
    centres.push(samples[weighted_pick(weights, &mut rng)]);

    let mut d2: Vec<f64> = samples
        .iter()
        .map(|s| delta_e(*s, centres[0]).powi(2))
        .collect();
    while centres.len() < k {
        let total: f64 = d2.iter().zip(weights).map(|(d, w)| d * w).sum();
        if total <= 1e-6 {
            break;
        }
        let mut target = rng.next_f64() * total;
        let mut chosen = samples.len() - 1;
        for (i, (d, w)) in d2.iter().zip(weights).enumerate() {
            target -= d * w;
            if target <= 0.0 {
                chosen = i;
                break;
            }
        }
        let centre = samples[chosen];
        centres.push(centre);
        for (d, s) in d2.iter_mut().zip(samples) {
            *d = d.min(delta_e(*s, centre).powi(2));
        }
    }
    centres
}

/// One index, chosen with probability proportional to its weight.
fn weighted_pick(weights: &[f64], rng: &mut DeterministicRng) -> usize {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return 0;
    }
    let mut target = rng.next_f64() * total;
    for (i, w) in weights.iter().enumerate() {
        target -= w;
        if target <= 0.0 {
            return i;
        }
    }
    weights.len() - 1
}

/// The centre nearest `colour`, by CIELAB ΔE.
fn nearest_lab(centres: &[[f64; 3]], colour: [f64; 3]) -> usize {
    centres
        .iter()
        .enumerate()
        .min_by(|a, b| delta_e(colour, *a.1).total_cmp(&delta_e(colour, *b.1)))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// A small, fixed-seed pseudo-random stream — splitmix64 — so k-means++'s
/// seeding is reproducible without a `rand` dependency for four lines of
/// arithmetic.
struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---------------------------------------------------------------------------
// CIELAB colour
// ---------------------------------------------------------------------------

fn srgb_to_linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f64) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The D65 white point, for the XYZ round trip either side of CIELAB.
const WHITE: [f64; 3] = [0.95047, 1.0, 1.08883];

/// **sRGB8 to CIELAB**, the space colour differences are perceptually close
/// to uniform in — which is the whole reason to cluster and measure distance
/// here rather than in raw RGB, where numerically close is not the same as
/// close to an eye.
pub fn srgb_to_lab(rgb: [u8; 3]) -> [f64; 3] {
    let r = srgb_to_linear(rgb[0]);
    let g = srgb_to_linear(rgb[1]);
    let b = srgb_to_linear(rgb[2]);

    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / WHITE[0];
    let y = (0.2126729 * r + 0.7151522 * g + 0.0721750 * b) / WHITE[1];
    let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / WHITE[2];

    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIELAB back to sRGB8, for turning a cluster centre into a paintable fill.
pub fn lab_to_srgb(lab: [f64; 3]) -> [u8; 3] {
    let fy = (lab[0] + 16.0) / 116.0;
    let fx = fy + lab[1] / 500.0;
    let fz = fy - lab[2] / 200.0;

    let finv = |t: f64| {
        let t3 = t * t * t;
        if t3 > 216.0 / 24389.0 {
            t3
        } else {
            (116.0 * t - 16.0) / (24389.0 / 27.0)
        }
    };
    let x = finv(fx) * WHITE[0];
    let y = finv(fy) * WHITE[1];
    let z = finv(fz) * WHITE[2];

    let r = 3.2404542 * x - 1.5371385 * y - 0.4985314 * z;
    let g = -0.9692660 * x + 1.8760108 * y + 0.0415560 * z;
    let b = 0.0556434 * x - 0.2040259 * y + 1.0572252 * z;

    [linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b)]
}

/// **ΔE\*ab** — Euclidean distance in CIELAB. The simplest of the ΔE
/// formulas, and close enough at the differences a tracer cares about: this
/// is deciding which side of a boundary a pixel is on, not certifying a
/// print proof.
pub fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Gradient magnitude of `l` (one value per pixel, row-major — CIELAB L* in
/// [`quantise`]'s case) by a 3×3 Sobel operator. Used to keep the palette
/// from being dragged toward the blended colours a soft edge is made of.
pub fn sobel_magnitude(l: &[f64], w: usize, h: usize) -> Vec<f64> {
    let at = |x: isize, y: isize| -> f64 {
        let x = x.clamp(0, w as isize - 1) as usize;
        let y = y.clamp(0, h as isize - 1) as usize;
        l[y * w + x]
    };
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (x as isize, y as isize);
            let gx = -at(xi - 1, yi - 1) - 2.0 * at(xi - 1, yi) - at(xi - 1, yi + 1)
                + at(xi + 1, yi - 1)
                + 2.0 * at(xi + 1, yi)
                + at(xi + 1, yi + 1);
            let gy = -at(xi - 1, yi - 1) - 2.0 * at(xi, yi - 1) - at(xi + 1, yi - 1)
                + at(xi - 1, yi + 1)
                + 2.0 * at(xi, yi + 1)
                + at(xi + 1, yi + 1);
            out[y * w + x] = (gx * gx + gy * gy).sqrt();
        }
    }
    out
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

    /// **The public ink mask is the line tracer's own**: a dark line on white
    /// paper is ink on the line and nowhere else, whether the threshold is found
    /// (Otsu) or given, and a speck under `speckle` pixels is cleaned away.
    #[test]
    fn ink_mask_marks_a_dark_line_and_nothing_else() {
        let (w, h) = (64usize, 40usize);
        // A 4 px high black bar across the middle (rows 18..22) of a white sheet,
        // and a 1 px speck far from it.
        let px = picture(w, h, |x, y| {
            if (18..22).contains(&y) || (x == 5 && y == 5) {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        for threshold in [None, Some(128u8)] {
            let ink = ink_mask(w as u32, h as u32, &px, threshold, 8);
            assert_eq!(ink.len(), w * h);
            // Away from the picture's border (the blur replicates it), the bar's
            // centre rows are ink and rows well off it are not.
            for x in 8..w - 8 {
                assert!(ink[19 * w + x] && ink[20 * w + x], "the bar is ink at x {x} ({threshold:?})");
                assert!(!ink[8 * w + x] && !ink[30 * w + x], "paper is not ink at x {x} ({threshold:?})");
            }
            assert!(!ink[5 * w + 5], "a one-pixel speck is cleaned away ({threshold:?})");
        }
        // Transparent pixels are paper, whatever colour they carry.
        let clear = picture(w, h, |_, _| [0, 0, 0, 0]);
        assert!(ink_mask(w as u32, h as u32, &clear, None, 8).iter().all(|&v| !v));
        // The automatic threshold is a luma between the two tones.
        let t = ink_threshold(w as u32, h as u32, &px);
        assert!(t > 0 && t < 255, "otsu threshold {t}");
    }

    /// **Local contrast finds a thin dark line on a mid-grey field that a
    /// global threshold cannot separate from the field.** A 3 px dark line
    /// across a grey (luma 128) field is ink under `LocalContrast { 0.62, 150,
    /// 21 }` and the grey is not; the global threshold 150 calls the whole grey
    /// field ink.
    #[test]
    fn local_contrast_marks_a_thin_line_on_grey_and_not_the_grey() {
        let (w, h) = (96usize, 64usize);
        let px = picture(w, h, |_, y| {
            if (30..33).contains(&y) {
                [20, 20, 20, 255]
            } else {
                [128, 128, 128, 255]
            }
        });
        let local = TraceOptions {
            ink_detect: InkDetect::LocalContrast { ratio: 0.62, max_luma: 150, window: 21, max_delta_e_to_line: f64::INFINITY },
            ..TraceOptions::default()
        };
        let ink = thin_ink_mask(w as u32, h as u32, &px, &local);
        for x in 10..w - 10 {
            assert!(ink[31 * w + x], "the line's centre is ink at x {x}");
        }
        for y in (0..h).filter(|y| !(28..35).contains(y)) {
            for x in 0..w {
                assert!(!ink[y * w + x], "the grey field is not ink at ({x}, {y})");
            }
        }
        let global = TraceOptions { ink_detect: InkDetect::Global(Some(150)), ..TraceOptions::default() };
        let all = thin_ink_mask(w as u32, h as u32, &px, &global);
        assert!(all[10 * w + 10], "a global threshold of 150 calls the grey field ink");
        // `Global(None)` still defers to `ink_threshold`.
        let legacy = TraceOptions { ink_threshold: Some(150), ..TraceOptions::default() };
        assert_eq!(thin_ink_mask(w as u32, h as u32, &px, &legacy), all);
    }

    /// **Ink is ink-coloured, judged per connected line (T9d).** A 3 px dark RED line
    /// (150, 30, 30) on cream (with a half-blended anti-aliased row each side) is
    /// locally dark but is not ink when the line colour is dark brown (40, 30, 25)
    /// and `max_delta_e_to_line` is 30: its component's median ΔE is far over the
    /// limit, so it is rejected WHOLE, edge rows included. A (60, 45, 40) line is kept
    /// WHOLE with its anti-aliased edge rows, although those edge pixels alone are
    /// further than 30 from the line colour (the per-pixel rule stripped them). With
    /// the test off (`INFINITY`) the red line is ink again.
    #[test]
    fn a_dark_red_line_is_not_ink_with_a_brown_line_colour_but_a_brown_one_is() {
        let (w, h) = (96usize, 64usize);
        let cream = [240u8, 230, 200];
        let line_at = |rgb: [u8; 3]| {
            picture(w, h, move |_, y| {
                if (30..33).contains(&y) {
                    [rgb[0], rgb[1], rgb[2], 255]
                } else if y == 29 || y == 33 {
                    // The anti-aliased edge: 40 % paper, 60 % line.
                    let mix = |c: usize| (0.4 * cream[c] as f64 + 0.6 * rgb[c] as f64).round() as u8;
                    [mix(0), mix(1), mix(2), 255]
                } else {
                    [cream[0], cream[1], cream[2], 255]
                }
            })
        };
        let options = |max_de: f64| TraceOptions {
            ink_detect: InkDetect::LocalContrast {
                ratio: 0.62,
                max_luma: 150,
                window: 21,
                max_delta_e_to_line: max_de,
            },
            line_colour: Some(Color::from_rgb8(40, 30, 25)),
            ..TraceOptions::default()
        };
        let red = line_at([150, 30, 30]);
        let ink = thin_ink_mask(w as u32, h as u32, &red, &options(30.0));
        assert!(ink.iter().all(|&i| !i), "a dark red line is not ink-coloured");
        let ink = thin_ink_mask(w as u32, h as u32, &red, &options(f64::INFINITY));
        assert!((10..w - 10).all(|x| ink[31 * w + x]), "with the test off the red line is ink");
        let brown = line_at([60, 45, 40]);
        let ink = thin_ink_mask(w as u32, h as u32, &brown, &options(30.0));
        assert!((10..w - 10).all(|x| ink[31 * w + x]), "a dark brown line is ink");
        let all = thin_ink_mask(w as u32, h as u32, &brown, &options(f64::INFINITY));
        assert!(all[29 * w + 40] || all[33 * w + 40], "the anti-aliased edge rows are candidates");
        assert_eq!(ink, all, "the brown line is kept whole, anti-aliased edge included");
    }

    /// **`ink_masses: false` keeps the strokes and drops the masses.** On white
    /// paper, a 40 x 40 dark square is not ink (its pixels are fatter than a
    /// line), but a 3 px dark outline round another 40 x 40 square is; the trace
    /// emits no mass for the square. With `ink_masses` on the square is traced.
    #[test]
    fn a_mass_is_not_ink_when_ink_masses_is_off_but_a_thin_outline_is() {
        let (w, h) = (130usize, 60usize);
        // The square at x 10..50, the outline of a square at x 80..120, y 10..50.
        let px = picture(w, h, |x, y| {
            let in_square = (10..50).contains(&x) && (10..50).contains(&y);
            let in_box = (80..120).contains(&x) && (10..50).contains(&y);
            let in_hole = (83..117).contains(&x) && (13..47).contains(&y);
            if in_square || (in_box && !in_hole) {
                [20, 20, 20, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let count = |ink: &[bool], x0: usize, x1: usize| {
            (0..h).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| ink[y * w + x]).count()
        };
        for detect in [
            InkDetect::Global(Some(128)),
            InkDetect::LocalContrast { ratio: 0.62, max_luma: 150, window: 21, max_delta_e_to_line: f64::INFINITY },
        ] {
            let strokes_only = TraceOptions { ink_detect: detect, ink_masses: false, ..TraceOptions::default() };
            let ink = thin_ink_mask(w as u32, h as u32, &px, &strokes_only);
            assert!(!ink[30 * w + 30], "the square's centre is not ink ({detect:?})");
            assert!(ink[11 * w + 100] && ink[30 * w + 81] && ink[48 * w + 100], "the outline is ink ({detect:?})");
            assert!(!ink[30 * w + 100], "inside the outline is paper ({detect:?})");
            assert!(count(&ink, 80, 120) >= 400, "the whole outline is ink ({detect:?})");
        }
        // Global detection: the square is a mass, so it is dropped (a few corner
        // slivers aside); local contrast keeps only its thin edge band, never the middle.
        let strokes_only =
            TraceOptions { ink_detect: InkDetect::Global(Some(128)), ink_masses: false, ..TraceOptions::default() };
        let ink = thin_ink_mask(w as u32, h as u32, &px, &strokes_only);
        assert!(count(&ink, 0, 60) <= 40, "the square leaves {} ink px", count(&ink, 0, 60));
        let with_masses = TraceOptions { ink_detect: InkDetect::Global(Some(128)), ..TraceOptions::default() };
        assert!(thin_ink_mask(w as u32, h as u32, &px, &with_masses)[30 * w + 30], "with masses on the square is ink");

        // The trace itself: strokes and no mass when off, a mass when on.
        let line_art = |ink_masses: bool| {
            trace(
                w as u32,
                h as u32,
                &px,
                &TraceOptions {
                    ink_detect: InkDetect::Global(Some(128)),
                    ink_masses,
                    ..TraceOptions::line_art()
                },
            )
        };
        let (off, on) = (line_art(false), line_art(true));
        assert!(off.strokes >= 1 && off.regions == 0, "{}", off.message);
        assert!(on.regions >= 1, "{}", on.message);
    }

    /// A white 300 x 160 sheet with two 3 px pen lines (the artist's pen, which sets
    /// `line_width_typical`) and every pixel `extra` says is dark.
    fn compactness_sheet(extra: impl Fn(usize, usize) -> bool) -> (usize, usize, Vec<u8>) {
        let (w, h) = (300usize, 160usize);
        let px = picture(w, h, |x, y| {
            let pen = (10..110).contains(&x) && ((120..123).contains(&y) || (140..143).contains(&y));
            if pen || extra(x, y) {
                [20, 20, 20, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        (w, h, px)
    }

    fn compactness_options() -> TraceOptions {
        TraceOptions { ink_detect: InkDetect::Global(Some(128)), ..TraceOptions::default() }
    }

    /// Whether any pixel of the box `x0..x1` x `y0..y1` is set in `mask`.
    fn any_in(mask: &[bool], w: usize, x0: usize, x1: usize, y0: usize, y1: usize) -> bool {
        (y0..y1).any(|y| (x0..x1).any(|x| mask[y * w + x]))
    }

    /// **T9e: a 12 x 60 dark bar (a brow) is a mass**, next to 3 px pen lines that stay lines.
    #[test]
    fn a_12_by_60_dark_bar_is_a_mass() {
        let (w, h, px) = compactness_sheet(|x, y| (10..70).contains(&x) && (10..22).contains(&y));
        let c = ink_components(w as u32, h as u32, &px, &compactness_options());
        assert!(c.mass_mask[16 * w + 40] && !c.line_mask[16 * w + 40], "the bar's centre is mass");
        assert!(c.mass_mask[12 * w + 20] && c.mass_mask[19 * w + 60], "the bar's body is mass");
        assert!(!any_in(&c.line_mask, w, 5, 75, 5, 30), "no part of the bar is a line");
        assert!(c.line_mask[121 * w + 60] && c.line_mask[141 * w + 60], "the pen lines are lines");
        assert!(!any_in(&c.mass_mask, w, 0, w, 110, 160), "the pen lines are no mass");
        assert!(c.line_width_typical >= 1.5 && c.line_width_typical <= 3.0, "pen {}", c.line_width_typical);
    }

    /// **T9e: a 3 px line 200 px long with a 6 px-thick 20 px stretch is ONE line**: a
    /// pressed stroke keeps its continuity, no part of it is a mass.
    #[test]
    fn a_pressed_stroke_stays_one_line() {
        let (w, h, px) = compactness_sheet(|x, y| {
            ((10..210).contains(&x) && (60..63).contains(&y)) || ((100..120).contains(&x) && (60..66).contains(&y))
        });
        let c = ink_components(w as u32, h as u32, &px, &compactness_options());
        assert!(!any_in(&c.mass_mask, w, 0, w, 0, h), "no mass anywhere");
        for x in 16..205 {
            assert!(c.line_mask[61 * w + x], "the stroke is a line at x {x}");
        }
        assert!(c.line_mask[64 * w + 110], "the pressed stretch is part of the line");
    }

    /// **T9e: a 10 px-radius disc (a pupil) is a mass.**
    #[test]
    fn a_ten_pixel_radius_disc_is_a_mass() {
        let (w, h, px) = compactness_sheet(|x, y| {
            let (dx, dy) = (x as f64 - 100.0, y as f64 - 50.0);
            dx * dx + dy * dy < 100.0
        });
        let c = ink_components(w as u32, h as u32, &px, &compactness_options());
        assert!(c.mass_mask[50 * w + 100] && !c.line_mask[50 * w + 100], "the disc's centre is mass");
        assert!(c.mass_mask[50 * w + 92] && c.mass_mask[44 * w + 104], "the disc's body is mass");
        assert!(!any_in(&c.line_mask, w, 85, 115, 35, 65), "no part of the disc is a line");
    }

    /// **T9e: a 3 px ring (an eye outline) is a line**, the whole ring.
    #[test]
    fn a_three_pixel_ring_is_a_line() {
        let (w, h, px) = compactness_sheet(|x, y| {
            let d = ((x as f64 - 150.0).powi(2) + (y as f64 - 60.0).powi(2)).sqrt();
            (11.0..14.0).contains(&d)
        });
        let c = ink_components(w as u32, h as u32, &px, &compactness_options());
        assert!(!any_in(&c.mass_mask, w, 0, w, 0, h), "no mass anywhere");
        for (x, y) in [(162, 60), (138, 60), (150, 72), (150, 48), (158, 68), (142, 52)] {
            assert!(c.line_mask[y * w + x], "the ring is a line at ({x}, {y})");
        }
        assert!(!c.line_mask[60 * w + 150], "the ring's inside is paper");
    }

    /// **T9e (the hand loops): thin enclosed paper slits between fused lines are filled,
    /// a ring's hole is not.** Three 3 px lines 3 px apart, joined at both ends, leave two
    /// enclosed slits 3 px wide: they become ink (one solid band, no loops to skeletonise).
    /// A 3 px ring keeps its hole.
    #[test]
    fn thin_slits_between_fused_lines_are_filled_but_a_rings_hole_is_not() {
        let (w, h, px) = compactness_sheet(|x, y| {
            let band = (10..60).contains(&x)
                && (10..25).contains(&y)
                && ((10..13).contains(&y)
                    || (16..19).contains(&y)
                    || (22..25).contains(&y)
                    || (10..13).contains(&x)
                    || (57..60).contains(&x));
            let d = ((x as f64 - 150.0).powi(2) + (y as f64 - 60.0).powi(2)).sqrt();
            band || (11.0..14.0).contains(&d)
        });
        let c = ink_components(w as u32, h as u32, &px, &compactness_options());
        let ink = |x: usize, y: usize| c.line_mask[y * w + x] || c.mass_mask[y * w + x];
        for (x, y) in [(30, 14), (30, 20), (20, 15), (50, 21)] {
            assert!(ink(x, y), "the slit pixel ({x}, {y}) is filled");
        }
        assert!(!ink(30, 27), "paper outside the band is paper");
        assert!(!ink(150, 60) && !ink(150, 52), "the ring's hole is paper");
        assert!(c.line_mask[60 * w + 162], "the ring is still a line");
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
        assert!(
            bb.center().x > 25.0 && bb.center().x < 39.0,
            "off centre: {bb:?}"
        );
    }

    /// **Keeping the paper keeps both.** The option is a choice, and a
    /// photograph needs its background.
    #[test]
    fn keeping_the_background_keeps_both_regions() {
        let px = disc(64, 64, 20.0);
        // `line_art()` now defaults to tracing centrelines (`TraceMode::LineArt`);
        // this test is about the region tracer specifically, so it asks for
        // `Outline` explicitly while keeping `line_art()`'s ink-and-paper
        // quantisation.
        let report = trace(
            64,
            64,
            &px,
            &TraceOptions {
                mode: TraceMode::Outline,
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
        // Region tracing specifically (see the note on
        // `keeping_the_background_keeps_both_regions`).
        let report = trace(
            w as u32,
            h as u32,
            &px,
            &TraceOptions {
                mode: TraceMode::Outline,
                ..TraceOptions::line_art()
            },
        );
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
        // Region tracing specifically (see the note on
        // `keeping_the_background_keeps_both_regions`) — speck-counting is
        // the region tracer's own concept.
        let report = trace(
            64,
            64,
            &px,
            &TraceOptions {
                mode: TraceMode::Outline,
                ..TraceOptions::line_art()
            },
        );
        assert_eq!(report.specks, 4, "{}", report.message);
        assert_eq!(report.regions, 1, "only the disc should survive");
    }

    /// Red square (`lo..hi` on both axes) on transparent, with 2x2 blue specks
    /// whose top-left corners are `specks`.
    fn red_square_with_specks(
        size: usize,
        lo: usize,
        hi: usize,
        specks: &[(usize, usize)],
    ) -> Vec<u8> {
        picture(size, size, |x, y| {
            let blue = specks
                .iter()
                .any(|&(sx, sy)| (sx..sx + 2).contains(&x) && (sy..sy + 2).contains(&y));
            if blue {
                [20, 40, 220, 255]
            } else if (lo..hi).contains(&x) && (lo..hi).contains(&y) {
                [220, 30, 30, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
    }

    /// **Specks inside a region merge into it, not leave holes.** Three 2x2
    /// blue specks inside a flat red square come back as one shape that
    /// covers the specks' centres, and the three are counted.
    #[test]
    fn specks_inside_a_region_merge_into_it_and_leave_no_hole() {
        let specks = [(100usize, 100usize), (60, 150), (150, 70)];
        let px = red_square_with_specks(240, 20, 220, &specks);
        let report = trace(
            240,
            240,
            &px,
            &TraceOptions {
                colours: 2,
                mode: TraceMode::Outline,
                drop_background: false,
                // Explicit, so the speck size does not depend on the internal
                // upscale: 20 picture pixels, far more than a 2x2 speck.
                speckle: 20,
                ..TraceOptions::default()
            },
        );
        assert_eq!(report.shapes.len(), 1, "{}", report.message);
        assert_eq!(report.specks, 3, "{}", report.message);
        let shape = &report.shapes[0];
        let rule = shape.fill.as_ref().map(|f| f.rule).unwrap_or(FillMode::NonZero);
        for &(sx, sy) in &specks {
            let p = Point::new(sx as f64 + 1.0, sy as f64 + 1.0);
            assert!(
                buzz_geom::hit::fill_contains_near(&shape.path, p, rule, 0.0),
                "a hole is left where the speck at {p:?} was: {}",
                report.message
            );
        }
    }

    /// **A speck with nothing around it is still dropped.** A lone 2x2 blue
    /// speck in transparent pixels has no region to join. (1024 wide, so the
    /// tracer does no internal upscale that would split the speck's edge.)
    #[test]
    fn an_isolated_speck_is_still_dropped() {
        let px = picture(1024, 16, |x, y| {
            if (500..502).contains(&x) && (7..9).contains(&y) {
                [20, 40, 220, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let report = trace(
            1024,
            16,
            &px,
            &TraceOptions {
                colours: 2,
                mode: TraceMode::Outline,
                drop_background: false,
                speckle: 20,
                ..TraceOptions::default()
            },
        );
        assert_eq!(report.shapes.len(), 0, "{}", report.message);
        assert_eq!(report.specks, 1, "{}", report.message);
    }

    /// **A speck joins the region it shares the longer border with.** A 3x3
    /// speck touches red along three sides and green along one: it goes to red.
    #[test]
    fn a_speck_joins_the_region_it_shares_the_longer_border_with() {
        let (w, h) = (12usize, 12usize);
        let in_speck = |x: usize, y: usize| (4..7).contains(&x) && (4..7).contains(&y);
        let in_green = |x: usize, y: usize| x == 7 && (4..7).contains(&y);
        let (mut red, mut green, mut speck) = (Vec::new(), Vec::new(), Vec::new());
        let mut region_of = vec![u32::MAX; w * h];
        for y in 0..h {
            for x in 0..w {
                let at = y * w + x;
                if in_speck(x, y) {
                    speck.push(at);
                } else if in_green(x, y) {
                    region_of[at] = 1;
                    green.push(at);
                } else {
                    region_of[at] = 0;
                    red.push(at);
                }
            }
        }
        let (red_before, green_before) = (red.len(), green.len());
        let mut regions = vec![(0u8, red), (1u8, green)];
        let mut record = Vec::new();
        let (merged, dropped) = merge_specks(
            &mut regions,
            &mut region_of,
            vec![(2u8, speck)],
            w,
            h,
            &mut record,
        );
        assert_eq!((merged, dropped), (1, 0));
        assert_eq!(record.len(), 1);
        assert_eq!((record[0].0, record[0].1.len()), (0, 9));
        assert_eq!(regions[0].1.len(), red_before + 9, "the red region did not grow by 9");
        assert_eq!(regions[1].1.len(), green_before, "green must be untouched");
        assert!(regions[0].1.contains(&(5 * w + 5)));
        assert_eq!(region_of[5 * w + 5], 0);
    }

    /// A 40x12 `host` rectangle with a 3x3 `speck` inside it, on a 1024x16
    /// transparent picture (1024 wide, so the tracer does no internal upscale
    /// and the speck is 9 cells at factor 1).
    fn host_with_3x3_speck(host: [u8; 4], speck: [u8; 4]) -> Vec<u8> {
        picture(1024, 16, |x, y| {
            if (498..501).contains(&x) && (6..9).contains(&y) {
                speck
            } else if (480..520).contains(&x) && (2..14).contains(&y) {
                host
            } else {
                [0, 0, 0, 0]
            }
        })
    }

    fn speck_options() -> TraceOptions {
        TraceOptions {
            colours: 2,
            mode: TraceMode::Outline,
            drop_background: false,
            speckle: 20,
            ..TraceOptions::default()
        }
    }

    fn fill_rgb(shape: &ShapeData) -> [u8; 3] {
        let [r, g, b, _] = shape
            .fill
            .as_ref()
            .expect("a filled shape")
            .color()
            .to_rgba8()
            .to_u8_array();
        [r, g, b]
    }

    /// **A contrasting speck is kept on top, and does not tint its host.** A
    /// 3x3 black speck (9 cells, at least 6) inside a red rectangle comes back
    /// as two shapes: the red host, coloured from its OWN cells only (the
    /// black ones merged into it would pull it darker), then the black speck.
    #[test]
    fn a_contrasting_speck_is_kept_on_top_and_does_not_tint_its_host() {
        let red = [220u8, 30, 30];
        let px = host_with_3x3_speck([red[0], red[1], red[2], 255], [0, 0, 0, 255]);
        let report = trace(1024, 16, &px, &speck_options());
        assert_eq!(report.shapes.len(), 2, "{}", report.message);
        assert_eq!(report.specks, 1, "{}", report.message);
        assert!(report.message.contains("kept 1 as detail"), "{}", report.message);

        let host = fill_rgb(&report.shapes[0]);
        for c in 0..3 {
            assert!(
                (host[c] as i32 - red[c] as i32).abs() <= 1,
                "the host is {host:?}, not its own red {red:?}: {}",
                report.message
            );
        }
        let speck = fill_rgb(&report.shapes[1]);
        assert!(
            speck.iter().all(|&c| c <= 8),
            "the speck on top should be black, got {speck:?}"
        );
        // The speck sits on top of the host and covers its own centre.
        let bb = report.shapes[1].path.bounding_box();
        assert!(
            bb.x0 >= 497.0 && bb.x1 <= 502.0 && bb.y0 >= 5.0 && bb.y1 <= 10.0,
            "the speck shape is at {bb:?}"
        );
    }

    /// **A speck close to its host's colour merges silently.** A 3x3 speck
    /// within 20 ΔE of the host's red, but different enough to be its own
    /// palette colour, adds no shape of its own.
    #[test]
    fn a_low_contrast_speck_merges_silently() {
        let host = [220u8, 30, 30];
        let speck = [205u8, 45, 40];
        let de = delta_e(srgb_to_lab(host), srgb_to_lab(speck));
        assert!((6.0..=20.0).contains(&de), "test colours are {de:.1} \u{394}E apart");
        let px = host_with_3x3_speck([host[0], host[1], host[2], 255], [speck[0], speck[1], speck[2], 255]);
        let report = trace(1024, 16, &px, &speck_options());
        assert_eq!(report.specks, 1, "the speck must be a speck first: {}", report.message);
        assert_eq!(report.shapes.len(), 1, "{}", report.message);
        assert!(!report.message.contains("detail"), "{}", report.message);
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
        assert!(
            bb.width() < 25.0 && bb.height() < 25.0,
            "traced the void too: {bb:?}"
        );
    }

    /// **The palette is asked for, not assumed.** Two colours from a two-colour
    /// picture, and never more than were asked for.
    #[test]
    fn the_palette_is_the_size_requested() {
        let px = disc(64, 64, 20.0);
        assert_eq!(quantise(&px, 64, 64, 2).len(), 2);
        // A picture with only two colours in it cannot yield eight.
        assert!(quantise(&px, 64, 64, 8).len() <= 8);
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

    // -----------------------------------------------------------------
    // T1 — colour trace fidelity
    // -----------------------------------------------------------------

    /// **Two regions traced independently still meet exactly.** A half-pixel
    /// outward dilation on every fill is the vector equivalent of the
    /// renderer's own seam seal: it is what keeps a hairline of background
    /// from showing between two shapes that came from two different flood
    /// fills of the same picture.
    #[test]
    fn adjacent_regions_leave_no_gap() {
        let (w, h) = (48usize, 64usize);
        // A soft, antialiased vertical seam: a few pixels of blend between
        // flat red on the left and flat blue on the right, the way a real
        // export's edge looks.
        let px = picture(w, h, |x, _| {
            let t = ((x as f64 - (w as f64 / 2.0 - 2.0)) / 4.0).clamp(0.0, 1.0);
            let r = (200.0 * (1.0 - t) + 20.0 * t) as u8;
            let b = (20.0 * (1.0 - t) + 200.0 * t) as u8;
            [r, 20, b, 255]
        });
        let report = trace(
            w as u32,
            h as u32,
            &px,
            &TraceOptions {
                colours: 2,
                mode: TraceMode::Outline,
                drop_background: false,
                ..TraceOptions::default()
            },
        );
        assert_eq!(report.regions, 2, "{}", report.message);
        assert_eq!(report.specks, 0, "{}", report.message);

        // Sample the seam the two regions share: every point along it must
        // be covered by one shape or the other, with nothing in between.
        let seam_x = w as f64 / 2.0;
        let mut uncovered = 0;
        for i in 0..h {
            let p = Point::new(seam_x, i as f64 + 0.5);
            let covered = report.shapes.iter().any(|s| {
                buzz_geom::hit::fill_contains_near(
                    &s.path,
                    p,
                    s.fill.as_ref().map(|f| f.rule).unwrap_or(FillMode::NonZero),
                    0.75,
                )
            });
            if !covered {
                uncovered += 1;
            }
        }
        assert_eq!(uncovered, 0, "background showed through the seam");
    }

    /// **A small but deliberate feature survives.** A 6-px dark dot on a
    /// skin disc is an eye: small against the whole picture, but nothing
    /// close to a speck, and it must still come back as its own shape.
    #[test]
    fn a_small_dark_dot_survives_on_a_larger_disc() {
        let (w, h) = (120usize, 120usize);
        let skin = [235u8, 195, 170];
        let eye = [30u8, 20, 20];
        let (cx, cy) = (60.0, 60.0);
        let (ex, ey, er) = (75.0, 55.0, 3.0);
        let px = picture(w, h, |x, y| {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let d_face = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            if d_face > 40.0 {
                return [0, 0, 0, 0];
            }
            let d_eye = ((fx - ex).powi(2) + (fy - ey).powi(2)).sqrt();
            if d_eye < er {
                [eye[0], eye[1], eye[2], 255]
            } else {
                [skin[0], skin[1], skin[2], 255]
            }
        });
        let report = trace(
            w as u32,
            h as u32,
            &px,
            &TraceOptions {
                colours: 6,
                mode: TraceMode::Outline,
                drop_background: false,
                ..TraceOptions::default()
            },
        );
        assert!(
            report.shapes.iter().any(|s| {
                let bb = s.path.bounding_box();
                bb.width() < 15.0 && bb.height() < 15.0 && bb.x0 <= ex && ex <= bb.x1 && bb.y0 <= ey && ey <= bb.y1
            }),
            "the eye did not survive: {}",
            report.message
        );
    }

    /// **A region is filled with its own colour**, not the palette centre it
    /// happened to cluster nearest to — two blobs sharing a palette entry
    /// because they are roughly the same colour must still come back
    /// individually correct.
    #[test]
    fn a_region_keeps_its_own_colour_not_the_cluster_average() {
        let (w, h) = (100usize, 60usize);
        let red_a = [200u8, 30, 30];
        let red_b = [140u8, 30, 90];
        let px = picture(w, h, |x, y| {
            if (10..30).contains(&x) && (10..50).contains(&y) {
                [red_a[0], red_a[1], red_a[2], 255]
            } else if (70..90).contains(&x) && (10..50).contains(&y) {
                [red_b[0], red_b[1], red_b[2], 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let report = trace(
            w as u32,
            h as u32,
            &px,
            &TraceOptions {
                colours: 2,
                mode: TraceMode::Outline,
                drop_background: true,
                ..TraceOptions::default()
            },
        );
        assert_eq!(report.regions, 2, "{}", report.message);

        let lab_a = srgb_to_lab(red_a);
        let lab_b = srgb_to_lab(red_b);
        for shape in &report.shapes {
            let bb = shape.path.bounding_box();
            let expect = if bb.x0 < 50.0 { lab_a } else { lab_b };
            let [r, g, b, _] = shape
                .fill
                .as_ref()
                .expect("a filled shape")
                .color()
                .to_rgba8()
                .to_u8_array();
            let got = srgb_to_lab([r, g, b]);
            assert!(
                delta_e(got, expect) <= 2.0,
                "region at {bb:?} filled {:?}, {:.2} \u{394}E from its own colour",
                [r, g, b],
                delta_e(got, expect)
            );
        }
    }

    /// **A square keeps its corners; a circle does not gain any.** The
    /// corner-aware smoothing must round a curve without rounding a corner
    /// off it — the whole reason `outline_colour` looks for them before it
    /// smooths.
    #[test]
    fn corner_aware_smoothing_keeps_squares_sharp_and_circles_round() {
        let square = picture(80, 80, |x, y| {
            if (20..60).contains(&x) && (20..60).contains(&y) {
                [20, 20, 20, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let report = trace(
            80,
            80,
            &square,
            &TraceOptions {
                colours: 2,
                mode: TraceMode::Outline,
                drop_background: true,
                smooth: 0.6,
                ..TraceOptions::default()
            },
        );
        assert_eq!(report.regions, 1, "{}", report.message);
        assert_eq!(
            count_corners(&report.shapes[0].path),
            4,
            "{}",
            report.message
        );

        let circle = disc(80, 80, 30.0);
        let report = trace(
            80,
            80,
            &circle,
            &TraceOptions {
                colours: 2,
                mode: TraceMode::Outline,
                drop_background: true,
                smooth: 0.6,
                ..TraceOptions::default()
            },
        );
        assert_eq!(report.regions, 1, "{}", report.message);
        assert_eq!(
            count_corners(&report.shapes[0].path),
            0,
            "{}",
            report.message
        );
    }

    /// Flatten `path` finely and count the sharp turns in it, the same way
    /// [`detect_corners`] finds them on a freshly traced loop.
    fn count_corners(path: &BezPath) -> usize {
        let mut points: Vec<Point> = Vec::new();
        kurbo::flatten(path.iter(), 0.1, |el| match el {
            buzz_geom::PathEl::MoveTo(p) | buzz_geom::PathEl::LineTo(p) => points.push(p),
            _ => {}
        });
        if points.len() > 1 && points.first() != points.last() {
            let first = points[0];
            points.push(first);
        }
        detect_corners(&points, 1.0).into_iter().filter(|&c| c).count()
    }

    #[test]
    fn two_colour_image_gives_one_shared_outline_plus_silhouette_at_uniform_width() {
        let px = picture(40, 40, |x, _| {
            if x < 20 {
                [200, 50, 50, 255]
            } else {
                [50, 50, 200, 255]
            }
        });
        let report = trace(
            40,
            40,
            &px,
            &TraceOptions {
                colours: 2,
                mode: TraceMode::ColourLines,
                uniform_width: Some(2.0),
                drop_background: false,
                ..TraceOptions::default()
            },
        );
        assert_eq!(report.regions, 2, "2 colour regions: {}", report.message);
        assert!(
            report.strokes >= 2,
            "expected outline strokes, got {}: {}",
            report.strokes,
            report.message
        );
        for shape in &report.shapes[report.regions..] {
            let stroke = shape.stroke.as_ref().expect("outline shape must have stroke");
            assert!(
                (stroke.width - 2.0).abs() < 1e-4,
                "stroke width must match uniform_width: {}",
                stroke.width
            );
        }
    }

    #[test]
    fn auto_picks_lineart_for_sketch_and_colourlines_for_flat_colour() {
        // 1. Black ink line on white paper (a sketch fixture)
        let sketch = picture(64, 64, |_, y| {
            if (31..=33).contains(&y) {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let sketch_report = trace(
            64,
            64,
            &sketch,
            &TraceOptions {
                mode: TraceMode::Auto,
                ..TraceOptions::default()
            },
        );
        assert!(
            sketch_report.strokes > 0,
            "Auto should have picked LineArt for sketch: {:?}",
            sketch_report.message
        );

        // 2. Flat colour fixture: 4 colour quadrants, no ink
        let flat_colour = picture(64, 64, |x, y| {
            match (x < 32, y < 32) {
                (true, true) => [220, 50, 50, 255],
                (false, true) => [50, 200, 50, 255],
                (true, false) => [50, 50, 220, 255],
                (false, false) => [220, 220, 50, 255],
            }
        });
        let colour_report = trace(
            64,
            64,
            &flat_colour,
            &TraceOptions {
                mode: TraceMode::Auto,
                colours: 4,
                drop_background: false,
                ..TraceOptions::default()
            },
        );
        assert_eq!(
            colour_report.regions, 4,
            "Auto should have picked ColourLines with 4 regions: {:?}",
            colour_report.message
        );
        assert!(
            colour_report.strokes > 0,
            "ColourLines should have produced outline strokes: {:?}",
            colour_report.message
        );
    }
}
