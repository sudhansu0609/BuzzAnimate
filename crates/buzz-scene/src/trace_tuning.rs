//! **Trace tuning** — the tracer's "look" decisions, as numbers you can change.
//!
//! Every value here used to be a constant in `trace_ink` / `trace_colour`.
//! [`TraceTuning::default`] is exactly those constants, so a trace with the
//! default tuning is the trace it always was. The values live on
//! [`crate::TraceOptions::tuning`]; the app keeps the user's own choice in
//! `trace_tuning.json` in its data folder ([`TraceTuning::path`]).
//!
//! Two environment variables reach every route that asks for the saved
//! tuning ([`crate::TraceOptions::with_saved_tuning`]: the Trace dialog, the
//! Character from Image wizard and the Assistant's `ai_character_from_image`,
//! the examples): `BUZZ_TRACE_TUNING=<file>` reads that file instead of the
//! app's own, and `BUZZ_TRACE_BOLD=<px>` replaces `TraceOptions::ink_bold`.
//!
//! Every field is `#[serde(default)]`, so a file written by an older or newer
//! build still loads: a missing field takes its default, an unknown one is
//! ignored. Each field's one-line doc is the hover text the UI shows.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// **Every adjustable tracer knob**: the ink stage's and the colour stage's.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TraceTuning {
    /// How the dark lines and masses are found and shaped.
    pub ink: InkTuning,
    /// How the flat colour fills under the ink are found.
    pub colour: ColourTuning,
}

/// **The ink stage** (`TraceMode::Ink`, and the ink of `InkAndColour`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InkTuning {
    /// Coverage at or above this is ink; higher keeps only the darker middle of a line, so lines come out thinner.
    // The anti-aliased edge of a line crosses 0.5 exactly where the line's true
    // edge is.
    pub ink_level: f32,
    /// How much brighter than the darkest ink a line's surroundings must be for it to count; higher drops lines drawn on dark cloth or hair.
    // A thin line only counts against its local background when that
    // background is at least this much brighter than the picture's own ink
    // core — otherwise the "background" is itself ink, and `f_local` is left 0.
    pub min_contrast: f64,
    /// A pixel this dark (luma, 0–255) or darker is ink outright; higher turns more dark cloth and shadow into solid ink.
    // A pixel this dark or darker is a dark mass outright, regardless of what
    // it sits on.
    pub dark_luma: f64,
    /// How many luma steps below the dark-ink level it takes to become fully ink; higher softens the edge of dark masses.
    // `f_global` ramps from 0 to 1 over this many luma below `dark_luma`.
    pub dark_ramp: f64,
    /// Which brightness of a pixel's neighbourhood counts as its background (0–1); higher finds lines against the lightest nearby colour.
    // The local background (step A.2) is the luma below which 80 % of the
    // opaque pixels in the window fall.
    // The 80th percentile, not the median: a line drawn on skin right next to
    // the hair (Raju's cheek line) has a window that is half dark mass, and the
    // median then calls the mass its background and loses the line. The 80th
    // percentile's own failure — a dark-cloth pixel within ~8 px of paper sees
    // "paper" as its background, so every cloth edge, sari border and bangle
    // became a band of ink — is handled by `remove_bands` instead.
    pub bg_percentile: f64,
    /// Half-width (px) beyond which a contrast-only dark band is a cloth edge, not a line; higher keeps thicker bands as ink.
    // A region that is ink only by LOCAL contrast (its absolute darkness is not
    // ink) and is thicker than twice this (px) is the edge of a mid-dark surface
    // against a lighter one — cloth beside paper, a red bangle on skin — not a
    // drawn line; such a region is dropped whole. Real lines are 2–4 px wide
    // (half-width ≤ 2), a hair or shoe mass is ink by absolute darkness.
    pub band_half_width_px: f64,
    /// A bend sharper than this many degrees stays a sharp corner; higher rounds off more corners.
    // A loop point turning more than this (step 13) is a corner. Also the
    // sharpest turn the curve fit may split a run at.
    pub corner_deg: f64,
    /// How far (px) the fitted curve may stray from the traced edge; higher gives fewer, looser curves.
    // The `kurbo::simplify` accuracy of the Bézier fit (step E).
    pub fit_tolerance_px: f64,
    /// Sub-pixel precision of the edge: Auto, or 1–6 cells per pixel; higher is a finer edge and a slower trace.
    // `None` is `U = 4` under 2.5 M source pixels, `2` under 10 M, else `1`;
    // `Some(u)` is clamped to 1..=6.
    pub upsample: Option<usize>,
    /// A small mark standing out from its surroundings by less than this (luma) is a faint crease and dropped; higher drops more faint marks.
    // A small component (under `faint_max_px2`) whose core pixels stand out
    // from their local background by less than this luma, and are not
    // themselves near-black, is a crease barely drawn into dark cloth: the eye
    // hardly sees it in the source, so a solid black dash for it is wrong.
    pub faint_max_contrast: f64,
    /// Only marks smaller than this (px²) can be dropped as faint; higher lets bigger faint marks go too.
    // See `faint_max_contrast`.
    pub faint_max_px2: f64,
    /// A gap inside ink up to this size (px²) is filled when it is dark; higher fills bigger dark gaps.
    // A hole of at most this many source px² whose source pixels are darker
    // than `dark_hole_max_luma` (median) is filled too: the lighter strands
    // painted into a hair mass, the belt showing through a buckle. Skin, paper
    // and eye whites are far lighter and stay holes; a trouser leg or a shirt
    // panel is far larger.
    pub dark_hole_max_px2: f64,
    /// A gap inside ink darker than this (luma) counts as dark and is filled; higher fills lighter gaps too.
    // See `dark_hole_max_px2`.
    pub dark_hole_max_luma: f64,
    /// A light mark more colourful than this is left for the colour fills, not ink; higher keeps more coloured marks as ink.
    // A component whose colour is lighter than luma 60 *and* more saturated
    // (max − min of RGB) than this — a red bindi, a bangle, a sari motif, a pen
    // cap — passed the coverage field by contrast alone and is left for the
    // colour stage. Dark reddish lines (a mouth, a cheek line at luma ≈ 40) are
    // ink and stay.
    pub colour_mark_min_chroma: f64,
}

impl Default for InkTuning {
    fn default() -> Self {
        Self {
            ink_level: 0.5,
            min_contrast: 60.0,
            dark_luma: 48.0,
            dark_ramp: 12.0,
            bg_percentile: 0.80,
            band_half_width_px: 3.0,
            corner_deg: 70.0,
            fit_tolerance_px: 0.15,
            upsample: None,
            faint_max_contrast: 40.0,
            faint_max_px2: 160.0,
            dark_hole_max_px2: 200.0,
            dark_hole_max_luma: 120.0,
            colour_mark_min_chroma: 60.0,
        }
    }
}

/// **The colour stage** (`TraceMode::InkAndColour`'s fills).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColourTuning {
    /// How many smoothing passes flatten shading before colours are picked; higher gives flatter, simpler fills.
    // Bilateral flatten (step 3): this many passes over a 7×7 window.
    pub bilateral_passes: usize,
    /// How different (ΔE) two colours may be and still be smoothed together; higher merges more shading into one tone.
    // The Gaussian range weight's σ (ΔE). Too high flattens small motifs into
    // their ground; too low leaves banding.
    pub bilateral_range_sigma: f32,
    /// Palette colours closer than this (ΔE) become one; higher gives fewer colours.
    // Palette centres closer than this (ΔE) are merged into one.
    pub palette_merge_de: f32,
    /// Most colours one flat surface may be split into; higher keeps more shades.
    // A surface's farthest-point seeding stops once every pixel lies within
    // `SURFACE_COVER_DE` of a centre, or once it has this many centres.
    pub surface_max_k: usize,
    /// A surface this grainy (median ΔE per px) or more is treated as a painterly wash; higher treats fewer surfaces as washes.
    // A surface whose MEDIAN source gradient (ΔE per px, on the under-painted
    // picture with the colour lines painted over) is at least this is a
    // painterly wash (Guddu's plaid shirt ≈ 3), not flat colour with cel
    // shading (skin ≈ 0.7, Raju's shirt ≈ 1, the sari ≈ 1). No flat
    // decomposition of a wash is right — every one is a patchwork — so a
    // textured surface is traced as a flat base plus its thin pattern lines
    // (the colour-line layer), keeping only large or strongly contrasting
    // patches.
    pub textured_median_gradient: f32,
    /// Most colours a painterly-wash surface may be split into; higher keeps more patches of the wash.
    // A textured surface's palette has at most this many centres (base plus two).
    pub surface_max_k_textured: usize,
    /// Neighbouring fills closer than this (ΔE) across a soft edge merge into one; higher merges more gradient bands.
    // Soft-boundary merge (anti-banding): adjacent regions whose mean colours
    // are closer than this (ΔE) and whose shared boundary is soft are one
    // surface with a gradient painted into it.
    pub soft_merge_max_de: f32,
    /// An edge sharper than this (ΔE per px) is never soft enough to merge across; higher lets busier patterns merge.
    // A busy pattern (a gold-on-red sari border) has a high interior gradient,
    // and without the cap its edge against the plain cloth beside it counted
    // as soft.
    pub soft_merge_gradient_cap: f32,
    /// Fills smaller than this (px²) are merged into a neighbour; higher removes more small patches.
    // A region smaller than this (px²) is merged into its most similar
    // 8-neighbour, unless it is fully enclosed by ink (an eye glint, a tooth).
    pub min_region_px2: usize,
    /// A small fill this different (ΔE) from all its neighbours is kept anyway; higher keeps fewer small marks.
    // A small region whose mean Lab is at least this ΔE from every 8-neighbour
    // region (a green leaf on a yellow sari, a glint) is never merged for being
    // small.
    pub contrast_region_min_de: f32,
    /// How different (ΔE) from its fill a thin stripe must be to become a pattern line; higher finds fewer pattern lines.
    // The residual's `LINE_DE`: lower than the local-median detector's, because
    // the comparison is against the region's own flat fill (no window effects),
    // and a plaid thread fades to ΔE 9–10 between its crossings — at 16 the
    // threads broke into dashes. ΔE 6.5 is the 0.5 crossing, 9.1 the 0.7 core.
    pub residual_line_de: f32,
    /// How many pixels deep the fills reach under the ink; higher runs fills under thicker lines.
    // Under-painting reaches this many pixels (8-connected layers) into the ink
    // and no further: the rim of a line or a mass takes its neighbours' colour,
    // so fills run under the ink's edges; the CORE of a mass (a belt, hair, a
    // pupil) keeps its own dark colour and becomes its own region. Painting
    // whole masses let the shirt's fill run under the entire belt, and in a
    // layered rig the chest drew that cream over the belt the hips drew below
    // it. The mask is the ink grown 1 px, so 4 layers paint a line up to 6 px
    // wide through; a mass 7 px or wider keeps its core.
    pub under_paint_max_layers: usize,
    /// How many pixels the ink is widened before fills are painted under it; higher hides more of the line's soft rim.
    // The native ink mask (`F ≥ ink_level`) is grown by this many pixels (a
    // square neighbourhood) so the anti-aliased rim — ink mixed into the colour
    // beside it — is under-painted too.
    pub ink_grow_px: usize,
    /// How far under an outline on the figure's edge the fill may reach: 0 stops at the line's inner edge, 0.5 at its centre, 1 at its outer edge.
    // An ink pixel (not a mass core) keeps its region iff
    // `d_int <= reach * (d_int + d_ext)`: `d_ext` is its distance to the
    // exterior (transparent, or a non-ink pixel of the dropped background
    // region), `d_int` to the nearest opaque non-ink pixel that is not
    // exterior. The vector ink is centred on the ink footprint, so at 1 (the
    // old behaviour) the fill under the outer outline showed as a sliver
    // outside the line.
    pub exterior_fill_reach: f32,
    /// Width (px) of the segmenter's soft edge outside the outline that is treated as background; 0 keeps it as fill.
    // Opaque non-ink pixels reachable from a transparent pixel through opaque
    // non-ink pixels within this distance of the transparency, with ink
    // within `FRINGE_INK_PX`: the matte's light anti-alias skirt outside a
    // line (luma ≈ 190), which otherwise became a thin light fill.
    pub fringe_px: f32,
    /// How close (ΔE) to the sheet's background colour a patch inside the figure must be to count as paper showing through (between hair curls or body-hair strands); 0 keeps every patch.
    // A6a (`docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md`): a light patch (pixels
    // within twice this ΔE of the background, 4-connected) whose mean colour
    // is within this ΔE, that is no larger than `background_pocket_max_px2`
    // and lies within `background_pocket_reach_px` of the outside (or touches
    // it), is transparent from step 1 on. The background is the picture's
    // border (Trace Bitmap) or the source sheet's (Character from Image).
    pub background_pocket_de: f32,
    /// A paper-coloured patch lying wholly within this distance (px) of the figure's edge, or touching the outside, is left out as background.
    pub background_pocket_reach_px: f32,
    /// Paper-coloured patches larger than this (px²) are always kept: a white shirt, a vest, a dhoti.
    pub background_pocket_max_px2: usize,
    /// Width (px) of the edge band where a colour between the outline's ink and the paper (the source's anti-aliasing, traced as grey dashes along hair tips) is left out as background; 0 keeps it.
    // A6d: an opaque pixel reachable from the outside through such pixels,
    // within this distance of the outside, whose Lab colour lies within
    // `blend_rim_de` of the segment from the darkest pixel within
    // `BLEND_RIM_INK_PX` (when that is ink-dark) to the sheet colour, at least
    // `blend_rim_min_t` of the way towards the paper. Same sheet colour and
    // protection as A6a; only with `drop_background`.
    pub blend_rim_px: f32,
    /// How far (ΔE) off the ink-to-paper blend an edge pixel may be and still count as the outline's anti-aliasing against the paper.
    pub blend_rim_de: f32,
    /// How far towards the paper (0 = the ink, 1 = the paper) an edge pixel's colour must be to count as the outline's anti-aliasing; darker stays with the ink.
    pub blend_rim_min_t: f32,
}

impl Default for ColourTuning {
    fn default() -> Self {
        Self {
            bilateral_passes: 2,
            bilateral_range_sigma: 10.0,
            palette_merge_de: 6.0,
            surface_max_k: 8,
            textured_median_gradient: 2.5,
            surface_max_k_textured: 3,
            soft_merge_max_de: 16.0,
            soft_merge_gradient_cap: 7.0,
            min_region_px2: 40,
            contrast_region_min_de: 20.0,
            residual_line_de: 13.0,
            under_paint_max_layers: 4,
            ink_grow_px: 1,
            exterior_fill_reach: 0.5,
            fringe_px: 1.5,
            background_pocket_de: 10.0,
            background_pocket_reach_px: 12.0,
            background_pocket_max_px2: 600,
            blend_rim_px: 3.0,
            blend_rim_de: 8.0,
            blend_rim_min_t: 0.3,
        }
    }
}

impl TraceTuning {
    /// `<app data dir>/BuzzAnimate/trace_tuning.json` (`%APPDATA%` on
    /// Windows), or the file named by `BUZZ_TRACE_TUNING` when that is set.
    pub fn path() -> PathBuf {
        if let Some(path) = std::env::var_os("BUZZ_TRACE_TUNING") {
            return PathBuf::from(path);
        }
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(std::env::temp_dir);
        base.join("BuzzAnimate").join("trace_tuning.json")
    }

    /// The tuning in the JSON file at `path`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))
    }

    /// The saved tuning ([`TraceTuning::path`]), or the defaults when there
    /// is no file or it cannot be read.
    pub fn load_or_default() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        Self::load(&path).unwrap_or_else(|e| {
            tracing::warn!("trace tuning: {e}; using the defaults");
            Self::default()
        })
    }

    /// Write this tuning to [`TraceTuning::path`] as pretty JSON, creating
    /// the folder if needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_json() {
        let tuning = TraceTuning::default();
        let json = serde_json::to_string_pretty(&tuning).unwrap();
        let back: TraceTuning = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tuning);

        let mut changed = TraceTuning::default();
        changed.ink.ink_level = 0.6;
        changed.ink.upsample = Some(2);
        changed.colour.min_region_px2 = 100;
        let back: TraceTuning = serde_json::from_str(&serde_json::to_string(&changed).unwrap()).unwrap();
        assert_eq!(back, changed);
    }

    #[test]
    fn an_unknown_or_missing_field_loads_as_default() {
        let empty: TraceTuning = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, TraceTuning::default());

        let partial: TraceTuning =
            serde_json::from_str(r#"{"ink": {"corner_deg": 90.0, "no_such_knob": 3}, "future_group": {}}"#).unwrap();
        let mut expected = TraceTuning::default();
        expected.ink.corner_deg = 90.0;
        assert_eq!(partial, expected);
    }
}
