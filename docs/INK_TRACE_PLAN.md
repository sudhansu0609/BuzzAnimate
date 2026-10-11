# Ink trace: outlines as smooth filled shapes (vectorizer.ai style)

User, 2026-10-06 (evening): "the tracing is not perfect, not usable still.
The outlines shall be more wide and dark and smooth. I want it done as
vectorizer.ai does it, clean, smooth. Let's start with just tracing the
outlines first, perfectly, and no colours in it."

Sources: `B:\youtubeProjects\Originals\series\ep1\<character>\*.png`
(ChatGPT T-pose sheets, 1086×1448 and similar). Rigs built from them live in
`B:\youtubeProjects\Originals\series\ep1\buzz\rigs\`.

## Status

| Step | State | Date | Evidence |
|---|---|---|---|
| Diagnosis + Python prototype of the ink field | done (Fable) | 2026-10-06 | below; scratch `proto_ink.py` masks of Raju / Guddu / Woman1 are clean line drawings at 4× (every outline, hair/brows/pupils/shoes as masses, no shading, no plaid) |
| I1 `trace_ink` module, `TraceMode::Ink`, `ink_bold` | done (implementer; awaiting I3 review) | 2026-10-06 | `cargo test --release -p buzz-scene` 518/518 green; `trace_ink::tests::{ring_and_line_trace_cleanly, ink_bold_grows_the_ring, smoke_pujari_front}` pass; ring area err 0.22 %, 31 elements (outer loop 4 cubics, inner 23). Deviation: step 16 uses `SimplifyOptions::default().angle_thresh(tan 70°)` — kurbo's default 1e-3 split every vertex, the fit was always rejected and DP gave 93 `LineTo`s |
| I2 dialog + agent + `trace_png ink` + 4× render | done (build-verified only; no render run yet) | 2026-10-06 | "Ink" first mode button, Ink shows Bold (0–3 px, 0.1) / Smoothness / Speck size only; agent `pick_named` has `Ink`; `trace_png … ink` + `TRACE_PNG_SCALE` (metrics skipped when ≠ 1); `cargo build --release -p buzz-app --example trace_png` and `-p buzz-ui` green |
| I3 Fable review on Raju / Guddu / Woman1 at 4× | **done — accepted for the outlines stage** | 2026-10-06 | Sheets in `B:\youtubeProjects\Originals\series\ep1\buzz\ink_check\` (`<name>_source_vs_ink.png`, `<name>_face_4x_source_vs_ink.png`, `raju_hand_4x…`). Fixes made during review (below): single-corner loops were dropped (`ring_slice`), `MIN_CONTRAST` 40 → 60, colour-mark gate, faint-mark gate, dark-hole fill. Raju 13 shapes / 1.26 s, Guddu 23 / 1.21 s, Woman1 22 / 1.29 s; `cargo test --release -p buzz-scene` 518/518; `-p buzz-ui` builds. By eye at 4×: face, eyes, hands, limbs are continuous solid smooth outlines, ~1 px wider than the source (bold 0.5), round caps, no staircase; hair/brows/pupils/shoes/belt solid. Known limit: hair whose base tone is as light as dark cloth (Woman1, luma ≈ 55) traces as its darker strands only — the colour stage (I4) paints that base. `buzzanimate.exe` not relinked (app running). |
| I3b refinement round: user "Raju is perfect, Guddu and Woman1 still need refinement" | **done** | 2026-10-06 | Guddu: plaid threads/stains no longer hairs and specks on the shirt (hysteresis), buttons kept; Woman1: sari border, bangles and blouse edges no longer bands (band rule), motifs gone; Raju unchanged by eye (cheek line, shoes, hair). Sheets refreshed in `ink_check/`. Left for the colour stage: Woman1's hair highlights (as light as skin), Guddu's shadowed waistband (luma 39–50, traces as a ragged strip), bangles as dark rings. |
| I4 colour fills under the ink (`TraceMode::InkAndColour`) | **Stage A3 (implementer, see "A3" under the I4 design): items 1–3 done — `under_paint` returns donors, `surfaces` + `surface_palette` + `surface_labels` (per-surface palettes, ink pixels take their donor's label), contrast exemption in `merge_regions`. Item 4: tests (a)–(d) and (f) `small_high_contrast_leaves_survive` pass; (e) `broad_plaid_bands_trace_as_strips` FAILS only on the count bound: 24 band fills (4 horizontal strips + 20 vertical segments, all ≥ 40 px²) vs 6–14 — the geometric minimum for crossing bands, needs an architect decision on the bound. `cargo test --release -p buzz-scene` 523 pass / 1 fail (that test); `-p buzz-app --example trace_png` and `-p buzz-ui` build. Item 5 (`trace_png … inkcolour`, scratch `<name>_inkcolour.png`): Raju 103 fills / 124 colour lines / 19 ink, mean ΔE 2.90, 2152 ms; Guddu 294 / 267 / 19, ΔE 5.08, 2163 ms; Woman1 616 / 175 / 33, ΔE 5.15, 2255 ms. Not reviewed by eye.** **Stage A2 (implementer, see "A2" under the I4 design): item 1 done — thin-stripe exemption needs area ≥ `THIN_STRIPE_MIN_PX2` = 20 (trace_colour tests 3/3 pass). Item 2 done — colour-line layer (`colour_line_coverage`, `colour_line_shapes`; trace_ink visibility only), tests (a)–(c) pass. Item 3: test (d) `plaid_threads_trace_as_colour_lines_over_a_plain_fill` FAILS only on `strokes ≥ 10`: "Traced 1 fills, 1 colour lines and 1 ink shapes" — the crossing threads are ONE 8-connected component, so one shape (base fill cream ✓, line orange ✓, no orange fill ✓; test uses `drop_background: false` since canvas + square are one cream region). Needs an architect decision (split threads, or assert ≥ 1). `cargo test --release -p buzz-scene` 521 pass / 1 fail (that test); `-p buzz-app --example trace_png` and `-p buzz-ui` build. Item 4 done (`trace_png … inkcolour` at 1×, scratch `<name>_inkcolour.png`): Raju 107 fills / 124 colour lines / 19 ink, mean ΔE 2.89, 2069 ms; Guddu 399 / 267 / 19, ΔE 4.97, 2013 ms; Woman1 569 / 175 / 33, ΔE 5.15, 2105 ms. Local background colour 31 ms on Raju (1.57 Mpx). Not reviewed by eye yet.** Stage A: **stage A in progress (implementer): items 1–3 done (segmentation, per-region vectorisation, output + `TraceMode::InkAndColour` / `ink_colour()` / dialog / agent / `trace_png inkcolour`); item 4 done: `trace_colour::tests::{disc_and_square_fill_with_their_own_colours, smooth_gradient_is_one_region, smoke_pujari_front_ink_and_colour}` pass (release); item 5 done: `cargo test --release -p buzz-scene` 521/521, `-p buzz-ui` and `-p buzz-app --example trace_png` build; Raju `trace_png … inkcolour` at 1×: 411 fills + 19 ink shapes, trace 1694 ms, mean ΔE 2.90, edge mean 0.31 px / p95 1.00 px (scratch `raju_inkcolour.png`). Awaiting Opus review; 411 fills vs the prototype's 137 is not explained yet (the thin-stripe exemption keeping edge slivers is the suspect). Stage B (shared-edge vectorisation) not started.** Earlier: designed; prototype validated | 2026-10-06 | user: "check how the colour can be implemented, it must be blotchless and perfect". Python prototype `proto_colour.py` (under-paint → mean-shift flatten → Lab k-means → regions → merge < 40 px² → median colours): Raju 137 regions and the composite is the source by eye at 1× and 3×; Woman1 300 regions (sari, border, blouse, motifs flat and clean); Guddu 549 (plaid as a coarse patchwork — the one soft spot). Design below. |
| I4 review (Fable) + stage A4 "better approach" | **done — accepted** | 2026-10-06 | User, after the A3 renders: "find better approach and continue". Sheets: `ink_check/<name>_inkcolour.png`, `_full.png`, `_face_3x.png`, `_torso_2x.png` (+ Raju `_shirt_2x`, Guddu `_shirt_4x`, Woman1 `_skirt_2x`). Raju ΔE 2.91 (91 fills, 180 colour lines, 19 ink), Guddu 5.11 (67 / 408 / 19), Woman1 5.19 (526 / 386 / 33); ~2.3 s each. By eye at 2–4×: Raju and Woman1 read as the sources (skin + cel shadow, hair, sari, border, blouse, motifs, pleats); Guddu's plaid reads as a plaid on its true light-tan ground (coarser than the source, blotch-free). Cracks: Raju 11 px, Woman1 ≈ 2 k px and Guddu ≈ 7.5 k px, all the soft ground shadow under the feet, which merges into the background by design. Test bounds I set wrong fixed: (d) asserts colour-line AREA ≥ half the drawn thread pixels (a crossing plaid is one component); (e) asserts 20–30 strips (4 + 4 × 5 is the geometric minimum). `cargo test --release -p buzz-scene` 524/524; `-p buzz-ui` builds. |
| Cast + episode rebuilt, app relinked | **done** | 2026-10-07 | `series_setup --replace --rigs-only`: 12 views, 0 failed (`ep1/buzz/rigs/*.png` previews, Cast assets replaced). `episode_build --scenes 4,5,6,7,10,12,14 --stills 3` → `ep1/buzz/episode_v3/ep1.buzz` (84 MB, 7 scenes, 21 stills, no warnings/skips in `report.md`). Stills by eye: all rigs in place with faces, patterns, continuous outlines (scene10 f0465, scene07 f0126, scene14 f0762 sunset). `cargo build --release -p buzz-app --bin buzzanimate` relinked (the app was not running). Known small artefacts: two short ink dashes left of Woman1 3-4's shoulder and a hairline sliver on Guddu's right shoulder — chest ink pieces on sleeve pixels the segmenter gave the chest, exposed when the arm drops; fix with the owner map, not the tracer. |
| I5 posing review (Cast previews, arms down) | **done — ghosts fixed** | 2026-10-07 | Posed previews `rigs_only_{Guddu,Raju}/rigs/*.png` (scratch): sleeves hang with the arms, no T-pose ghost on Raju, a hairline sliver on Guddu's right shoulder. Fixes: hidden completion limited to `HIDDEN_REACH_FRACTION` of owned pixels; remainder split by nearest bone (`split_remainder_by_bone`); see "I5 posing review" below. All suites green (buzz-scene 524, buzz-ai lib 75, trace_fidelity 23, trace_character 3, quality_redesign 10, face_rig 5); `a_covered_part_gets_a_hidden_fill…` now expects the reach-limited strip. |
| I5 Fable review + fixes | **done — accepted** | 2026-10-07 | Sheets `ink_check/<name>_source_vs_inkcolour_vs_rig.png` (+ `raju_face_3x…`, `raju_shoe_3x…`): the rigs of Raju, Guddu and Woman1 read as their sources — face (white, pupil, glint, lid line), continuous outlines, belt and shoes in their own brown with a dark rim, no stray bars/squares/slivers. `rig_vs_source`: Raju ΔE 2.05 / edge p95 1.0 / ink coverage 99.3 % / 53 ink shapes; Guddu 4.29 / 1.41 / 98.5 %; Woman1 4.41 / 1.41 / 98.0 %. All suites green: buzz-scene 524, buzz-ai lib 75, trace_fidelity 23, trace_character 3, quality_redesign 10, face_rig 5. Fixes below ("I5 review"). `series_setup --replace --rigs-only` running (12 rigs); then `episode_build`; `buzzanimate.exe` relink needs the app closed. |
| I5 character pipeline uses Ink + Colour (`trace_character_whole_with`) | implementer stage (superseded by the review row above) | 2026-10-06 | `TraceCharacterOptions::ink_colour` (default true; only with `outline_strokes` off) → one `TraceOptions::ink_colour()` trace replaces the T9e pair; ink shapes are ink only at ≥ `INK_LINE_SHARE` 0.60 of their area on `comps.line_mask` (`line_share`), else tiled as fills; T9e path unchanged. `series_setup` / `buzz-ui` already use `..Default::default()`; `trace_fidelity.rs` got it too. Tests (release): `-p buzz-ai --lib` 75/75, `trace_character` 3/3, `quality_redesign` 10/10, `trace_fidelity` 21/23. Switched to `ink_colour: false` (helper `trace_t9e`, one-line comment each; they hold to the T9 Outline region, the LineArt ink, the T9d protected eye or the T9b under-paint): (d) mostly_inside…, (f) sixty_percent_torso…, (h) clipped_pieces_tile…, (k) ink_outline_crossing…, (m) drawn_eye_in_the_protected_box…, (m2) drawn_eye_owned_by_an_eye_part…, (p) under_paint_leaves_a_brow_mass…, (n) straddling_three_parts…. **Still failing (picture, cause is the `InkAndColour` tracer in buzz-scene, not fixable in buzz-ai under the constraints):** (a) `plaid_grid_regions_survive_as_fills` — the fixture's 40 dark-red 3×11 px dashes are absent from the tracer's report (0 of 40); (b) `every_fill_is_the_exact_source_colour_under_it` — 13 fills (needs > 40, the dashes), torso fill ΔE 7.3 from the source mean under it (it covers the lost dashes), and a 49 px pink colour-line fragment at the torso/arm cut (body 136..145 × 132..148) is ΔE 17.2 off the source. Also seen: the tracer emits 34 px corner fragments of each flat patch as "colour lines" (the extra pieces in d/h/n); under ink_colour (k) the chest ink spills 89 of 1168 px > 3 px into the arm (T9e bound 2 %). Eye fixture under ink_colour: white fill 100 %, ring ink 100 %, pupil ink 100 % (the r = 5 pupil is on `line_mask`). Item 6 `rig_vs_source` Raju (scratch `rigs_i5`): mean ΔE 2.69 (**worse than T9e's 1.77**), edge mean 0.42 / p95 1.41 px, small features 116/178 (65.2 %), ink coverage 97.45 %, 472 shapes, only 5 ink shapes (most tracer ink shapes fall under the 60 % line share and are tiled as fills), pipeline 6.54 s. Not reviewed by eye; Cast not rebuilt. |

### I3b changes (Fable, 2026-10-06)

- **Hysteresis** (`require_core`, `SEED_LEVEL 0.7`): a pixel is ink only with a ≥ 0.7 pixel in its 3×3 neighbourhood. Real lines have a solid core beside their anti-aliased edge; the darker threads of Guddu's plaid, the vest stains and faint creases peak at 0.55–0.65 and vanish whole. (Raising `INK_LEVEL` to 0.65 instead was tried: it also lost the lighter half of Raju's cheek line and the shirt buttons — reverted.)
- **Band rule** (`remove_bands`, `BAND_HALF_WIDTH_PX 3`): a region that is ink by local contrast only and is deeper than 3 px is the edge of a mid-dark surface against a lighter one (trousers beside paper — the 80th-percentile background sees paper within ~8 px of the edge — a sari border, a red bangle), not a line; dropped whole. The median background was tried instead and lost lines next to dark masses (the cheek line beside the hair) — reverted to the 80th percentile.
- **Colour pixels** (`COLOUR_PIXEL_MIN_LUMA 60`, `COLOUR_PIXEL_MIN_CHROMA 140`): bright saturated pixels never count as local-contrast ink. A bar of 100 ate the ink/skin anti-alias mix (chroma ≈ 120) — 140.
- **Mass chroma** (`MASS_MAX_CHROMA 100`): the absolute-darkness rule only makes ink of near-neutral pixels (hair, shoes, pupils ≤ 50); the shaded mid-tones of a red bangle (138, 12, 11; chroma 128) are a colour.
- **Fit guard** (`FIT_MARGIN_PX 2`): a fitted run is accepted only if the curve, sampled along every segment, stays within the run's bounds + 2 px (kurbo once returned a cubic swinging 360 px below the picture — a stray line through Raju's face). Control points are not checked (a wide arc's legitimately sit outside).
- Tried and reverted (see the comment in `trace_ink.rs`): dropping absolute-dark regions with < 25 % truly-dark pixels (punched gaps into Raju's shoes and hair); a 3×3 median on the darkness input (thin lines lost their absolute coverage and the band rule then swallowed them with the cloth edges they touch — Guddu's shirt went dashed).

### I3 review findings and changes (Fable, 2026-10-06)

- **Bug:** a loop with exactly one corner lost its whole run — `ring_slice(start, end)` with `start == end` returned one point, so every hole bounded by lines meeting once (the face interior, 28 of Raju's 98 loops) vanished and the face rendered solid. Fixed: `start == end` walks the full loop.
- **Exporter limit:** `TRACE_PNG_SCALE=4` on a full 1086×1448 picture renders black (canvas too large for the exporter, Outline mode too). Added `TRACE_PNG_CROP=x0,y0,x1,y1` to render a window at 4× instead; `examples/ink_debug.rs` prints per-shape subpath counts and signed areas; `BUZZ_INK_DEBUG_DIR=<dir>` dumps the U× mask and per-loop stats.
- **`MIN_CONTRAST` 40 → 60:** on the luma-57 trousers (`bg − core` ≈ 40) the local rule just passed, so the faint creases (luma 40–45) came out as solid dashes. Lines on dark cloth are near-black anyway and the absolute rule keeps them.
- **Colour marks are not ink** (`is_colour_mark`): a component whose core colour is lighter than luma 60 *and* has RGB chroma > 60 (bindi, bangles, sari motifs, pen caps) is left for the colour stage. Woman1 went from 127 to 22 shapes; dark reddish lines (mouth, cheek, luma ≈ 40) stay.
- **Faint marks are not ink** (`is_faint_mark`): a component under 160 px² whose core pixels stand out from their local background by < 40 luma and are not near-black (> 30) is a barely-drawn crease; dropped. Raju 16 → 13 shapes, nothing visible lost (belt loops, buckle, buttons, watch, bag lines all kept).
- **Dark holes are filled** (`fill_holes`): a paper hole ≤ 3 px² always; a hole ≤ 200 px² whose source pixels have median luma < 120 (a lighter strand painted into hair, the belt through a buckle) too. Eye glints and skin stay holes.
- Constants now: `BG_PERCENTILE 0.80`, `MIN_CONTRAST 60`, `DARK_LUMA 48`, `DARK_RAMP 12`, `CORE_PERCENTILE 0.01`, `CORNER_DEG 70`, fit accuracy 0.15, `MAX_HOLE_PX2 3`, `DARK_HOLE_MAX_PX2 200`, `DARK_HOLE_MAX_LUMA 120`, `FAINT_MAX_PX2 160`, `FAINT_MAX_CONTRAST 40`, `FAINT_MIN_LUMA 30`, `COLOUR_MARK_MIN_LUMA 60`, `COLOUR_MARK_MIN_CHROMA 60`.

## Diagnosis (measured 2026-10-06, Raju source)

- The ink is a **2–3.5 px dark-brown line** (core luma 0–20, colour ≈
  (56, 43, 35)) with one anti-aliased pixel each side (luma 98–120 against
  skin 175 / paper 251). Across the jaw at row 230: `… 250, 243, 119, 0, 15,
  98, 178 …`.
- Dark masses: hair luma 31, shoe 38, belt 36, pupils/brows ≈ 10–30. The
  **trousers are luma 57** (RGB 69, 54, 46), only 20 above the darkest masses;
  their crease lines are luma 45 (contrast 12 — nearly invisible in the
  source).
- `TraceMode::LineArt` (skeleton → brush): strokes of uneven width, blotches
  at junctions, lines missed where Otsu swallows them (`cur_lineart_face4x`).
- `TraceMode::Outline` (region trace): every edge pixel-stepped, thin lines
  broken, colours quantised (`cur_outline_face4x`).
- Neither resembles vectorizer.ai, which traces the **ink as a filled
  region** with a sub-pixel boundary and fits smooth Béziers with corners
  kept — the stroke keeps its true width everywhere, caps are round,
  junctions are solid.

## What "right" is

At 4× zoom the traced ink lies on the source ink: same width (plus the
chosen bold), continuous through every junction, round caps, no staircase,
no wobble, no width pumping; hair/brows/pupils/shoes solid; nothing traced
in the shirt plaid, skin shading or cloth; the trousers (luma 57) are not
ink. Output: a handful of filled shapes (one per connected ink component,
holes included), fill colour = the ink's own core colour (dark brown here).

## I1 — `crates/buzz-scene/src/trace_ink.rs` (one implementer run)

Entry `pub(crate) fn trace_ink(width: u32, height: u32, pixels: &[u8],
options: &TraceOptions) -> TraceReport`, dispatched from `trace::trace` for
the new `TraceMode::Ink`. Everything deterministic (no HashMap iteration
order in the output).

### A. Ink coverage field (native resolution, `f32` per pixel)

1. `l` = luma over white: `(0.299 R + 0.587 G + 0.114 B)·a + 255·(1 − a)`,
   `a = alpha/255`. **No blur** (blur widens the anti-alias ramp and moves
   the edge). `opaque = alpha > 128`.
2. Local background `bg` = the **80th-percentile luma of the opaque pixels in
   a `W×W` window** centred on the pixel, `W = odd(clamp(round(max(w,h)/55),
   15, 41))` (Raju: 27). Refactor the sliding-histogram loop out of
   `trace_lines::ink_mask_local` into
   `pub(crate) fn local_background(w, h, bins: &[u8], opaque: &[bool], window: usize, percentile: f64) -> Vec<u8>`
   and have `ink_mask_local` call it (its behaviour unchanged: it keeps
   passing *blurred* bins). Windows with no opaque pixel → 255.
3. `core` = the **1st percentile** of `l` over opaque pixels (Raju 18).
4. Per pixel:
   - `f_local = if bg − core ≥ 40 { clamp((bg − l)/(bg − core), 0, 1) } else { 0 }`
     — the thin dark line against whatever it is drawn on (skin, paper,
     shirt, trousers). Inside a dark mass the window background is itself
     dark, so this is 0 there.
   - `f_global = clamp((48 − l)/12, 0, 1)` — the dark masses (hair 31, shoe
     38, belt 36 → 1; trousers 57 and their creases 45 → 0).
   - `F = if opaque { max(f_local, f_global) } else { 0 }`.
   Constants (module-level, documented): `BG_PERCENTILE = 0.80`,
   `MIN_CONTRAST = 40.0`, `DARK_LUMA = 48.0`, `DARK_RAMP = 12.0`,
   `CORE_PERCENTILE = 0.01`.

### B. Sub-pixel binary mask at `U×`

5. `U = 4` when `w·h ≤ 2.5 M`, `2` when `≤ 10 M`, else `1`. Mask
   `B: Vec<bool>` of `(U·w) × (U·h)`; `B[X,Y] = F_bilinear(((X+0.5)/U − 0.5,
   (Y+0.5)/U − 0.5)) ≥ 0.5`, sample coordinates clamped into
   `[0, w−1] × [0, h−1]`. Do not materialise `F` at `U×`; threshold on the
   fly, rows in parallel (rayon).
6. Clean: `trace_lines::clean(&mut B, Uw, Uh, options.speckle · U²)` (ink
   specks under `speckle` source px² go, paper pinholes ≤ 4 cells are
   filled).
7. **Bold** (`options.ink_bold` px each side, default 0.5): `r = round(ink_bold
   · U)` cells. When `r > 0`: collect every ink cell with a non-ink
   4-neighbour (from the *undilated* mask), then for each set all cells with
   `dx² + dy² ≤ r²` around it. (Boundary cells only — a few hundred thousand
   at 4× — so this is cheap; no EDT at `U×`.)

### C. Contours, grouped by component

8. Label the 8-connected components of `B` (`Vec<u32>`, 0 = paper).
9. Build a `bucket::Grid` from `B` and call `bucket::trace_contours(&grid)`
   once: closed loops of grid-corner points, outer loops clockwise on screen,
   holes the other way (so `FillMode::NonZero` renders holes as holes —
   exactly as `trace.rs::trace_regions` relies on). Drop loops with fewer than
   `8` points.
10. Each loop belongs to the component of **the ink cell on the right of its
    first edge**: with `p = loop[0]`, heading `d` from `loop[1] − loop[0]`
    (0 right, 1 down, 2 left, 3 up), the cell is `d=0: (p.x, p.y)`,
    `d=1: (p.x−1, p.y)`, `d=2: (p.x−1, p.y−1)`, `d=3: (p.x, p.y−1)`.
11. Convert to source pixels: divide by `U` (`f64`). Points are then spaced
    `1/U` px apart along the loop.

### D. Smooth with corners kept

Per loop (closed polyline `P`, `n` points), all sigmas in points:

12. Pre-smooth with a **circular Gaussian**, `σ₀ = 0.5·U` (0.5 px), kernel
    radius `3σ`.
13. **Corners**: `k = round(0.75·U)`; angle at `i` between `P[i] − P[i−k]` and
    `P[i+k] − P[i]` (indices wrap). `i` is a corner iff angle `> 70°` and it is
    the maximum over `[i−k, i+k]`. Loops with `n < 4k` have no corners. (A
    3.5 px round cap turns ~50° per 1.5 px of arc → not a corner; the notch
    where two lines meet turns ~90° within half a pixel → corner.)
14. **Main smoothing** `σ₁ = options.line_smooth · 2 · U` (default 0.5 →
    1 px = 4 points at 4×), kernel radius `3σ₁`:
    - no corners: circular Gaussian over the whole loop;
    - with corners: each run between consecutive corners (endpoints
      inclusive) is smoothed as an open polyline with **reflected padding**
      about its endpoints (`P[−j] = 2·P[0] − P[j]`), which pins the corner
      positions and keeps the tangents; the corner points themselves are not
      moved.
15. For a corner-less loop, rotate the start to the point of **least turning**
    (smallest `|angle|` from step 13 recomputed with `k`), so the seam of the
    fit lies on the straightest part.

### E. Curve fit

16. Per run (or per whole loop when corner-less), build a `kurbo::BezPath`
    (`MoveTo`, `LineTo…`) and call
    `kurbo::simplify::simplify_bezpath(path, 0.15, &kurbo::simplify::SimplifyOptions::default())`.
    Accept the result when every coordinate is finite and it has no more
    elements than the input; otherwise fall back to Douglas–Peucker of the
    run at `ε = 0.2 px` as `LineTo`s (`bucket::douglas_peucker` is private —
    add a tiny `pub(crate)` one on `Point`s in `trace_ink.rs`).
17. Concatenate runs into **one closed subpath per loop**: keep the first
    `MoveTo`, drop each later run's `MoveTo` (its start is the previous end),
    finish with `ClosePath`. One `BezPath` per component holds all its loops.

### F. Colour and output

18. Per component: for every **native** pixel with `F ≥ 0.9`, look up the
    label of cell `(U·x + U/2, U·y + U/2)`; the component's colour is the
    per-channel **median** of those pixels' RGB (Raju → ≈ (56, 43, 35)); fewer
    than 8 samples → the darkest pixel under the component. `options.line_colour
    = Some(c)` overrides every component with `c`.
19. `ShapeData::filled(path, colour)` (NonZero), components sorted by
    `|area|` descending. `TraceReport { shapes, regions: n, specks: dropped,
    message: "Traced {n} ink shapes" }` (`strokes`/`fills` 0).
20. Performance: Raju (1086×1448) ≤ 1.5 s release. Memory at 4×: ~25 M bools
    + 25 M `u32` labels is accepted.

### Options and plumbing (`trace.rs`)

- `TraceMode::Ink` — doc: "Dark ink as smooth filled shapes: outlines only,
  the way vectorizer.ai traces a line drawing. See `trace_ink`."
- `TraceOptions::ink_bold: f64` (default `0.5`; "pixels added to each side of
  every ink shape; 0 keeps the source width").
- `TraceOptions::ink()` constructor: `mode: Ink, speckle: 12, line_smooth:
  0.5, ink_bold: 0.5, drop_background: true, ..default()`.
- `pub fn ink_field(width, height, pixels) -> Vec<f32>` in `trace.rs`
  (wrapper over step A, for the character pipeline later).
- Everything that matches on `TraceMode` exhaustively compiles (`trace()`,
  dialog, `agent/dialogs.rs:325` list gets `TraceMode::Ink`).

### Tests (in `trace_ink.rs`, `#[cfg(test)]`)

- **Ring + line**: 200×200 white canvas; draw, with 4× supersampled
  coverage, an annulus centre (100, 100), outer radius 40, inner 36.5, colour
  (56, 43, 35), and a straight 3 px line from (20, 180) to (180, 185). Trace
  with `TraceOptions::ink()` and `ink_bold = 0`: exactly 2 shapes; the ring
  shape's path has 2 loops (count `MoveTo`s), `|kurbo::Shape::area|` within
  6 % of `π(40² − 36.5²)`, ≤ 40 path elements, every coordinate finite; its
  fill colour within 6/255 per channel of (56, 43, 35). With `ink_bold = 0.5`
  the ring area grows by `≈ 0.5 · (2π·40 + 2π·36.5)` (±20 %).
- **Smoke**: `../buzz-ai/tests/data/pujariFront_0.png` (346×1024 RGBA): ≥ 1
  shape, every coordinate finite, under 2 s.
- `cargo test --release -p buzz-scene` green; `cargo build --release -p
  buzz-app --example trace_png` green.

## I2 — dialog, agent, example

- `buzz-ui/src/trace_dialog.rs`: a sixth mode button "Ink" (first in the row);
  when `mode == Ink` show only: **Bold** (`ink_bold`, slider 0–3 px, 0.1
  step), **Smoothness** (`line_smooth`), **Speck size**; the colour and
  line-art rows are hidden.
- `buzz-app/src/agent/dialogs.rs`: `TraceMode::Ink` in the `pick_named` list.
- `buzz-app/examples/trace_png.rs`: mode `ink` → `TraceOptions::ink()`; env
  `TRACE_PNG_SCALE` (default 1) renders the vectors at that scale
  (`ExportSettings::scaled(&scene, s)`) so 4× crops can be judged.

## I4 — colour fills under the ink (design, 2026-10-06)

What vectorizer.ai does with a coloured cartoon: segments it into flat
regions, fits every region boundary **once, shared between its two
regions** (so fills are watertight — no hairline cracks, no overlaps), with
the same sub-pixel edge and curve fit as the outlines. "Blotchless" means:
no speck regions, no banding where the source has a soft gradient, no
patchwork inside a flat surface, and the colour of each region is the
source's own.

Prototype (`proto_colour.py`, scratch) proved the segmentation half: Raju's
composite (fills + ink) is the source by eye; Woman1's sari, border, blouse
and motifs come out as clean flat regions; Guddu's plaid comes out as a
coarse patchwork of 3–4 tones (acceptable, improvable by the thin-stripe
rule in step 5).

### Pipeline (new `crates/buzz-scene/src/trace_colour.rs`, one implementer run per step group)

1. **Ink first.** Run `trace_ink`'s analysis to get the coverage field `F`
   and the final ink mask at native resolution (grow it by 1 px so the
   anti-aliased rim, ink mixed into colour, counts as ink too).
2. **Under-paint.** Every ink pixel takes the colour of the nearest non-ink
   pixel (EDT with indices; `buzz-ai`'s `paint_under_ink` does this already
   — move the core of it into buzz-scene). Fills then run *under* the lines,
   and a slightly thinner stroke never shows a dark halo.
3. **Flatten (edge-preserving).** Two passes of a bilateral filter in Lab
   (spatial σ 3 px, range σ 10 ΔE; window 7×7) on the under-painted
   picture. This is what makes it blotchless: airbrushed shading inside a
   surface collapses to one tone while every real colour edge stays put.
   (The prototype used OpenCV mean shift; bilateral is the Rust-friendly
   equivalent.)
4. **Palette and labels.** k-means in Lab on the flattened opaque pixels
   (`K = 24`, k-means++ seeds, 20 iterations; the existing `quantise` may
   seed it), then merge centres closer than ΔE 6. Every pixel gets the
   label of its nearest centre.
5. **Regions.** 8-connected components per label. Merge a region into its
   most similar neighbour (ΔE of mean colours) when it is smaller than
   `40 px²` — unless it is fully enclosed by ink (an eye glint, a tooth),
   where the floor is `4 px²`. **Soft-boundary merge** (anti-banding): two
   adjacent regions with ΔE < 8 whose shared boundary has a low source
   gradient (mean |∇Lab| along it < 4 ΔE/px) are one surface with a
   gradient painted into it — merge them. **Thin stripes** (plaid, sari
   pleats): a region thinner than 3 px everywhere (EDT max < 1.5) keeps its
   own label even when small, so a plaid thread stays one stripe instead
   of dissolving into dots.
6. **Sub-pixel label map at `U×`** (same `U` as the ink). Bilinearly
   resample the *flattened* colour to `U×`; each cell takes the label of the
   nearest palette colour **among the labels present in the 3×3 native
   neighbourhood** (so no far label leaks in). The interpolated colour
   crosses the midpoint between two neighbours exactly at the anti-aliased
   edge, which is the sub-pixel boundary for free. Cells that are ink at
   `U×` keep the under-painted label (fills run under the ink).
7. **Shared-edge vectorisation (watertight).** Build the boundary graph of
   the label map: junction corners (≥ 3 distinct labels around a grid
   corner) and chains of grid edges between junctions, each chain separating
   exactly two labels; closed chains with no junction are loops. Smooth each
   chain **once** as an open run with pinned endpoints (`gaussian_smooth_open`,
   σ₁ as the ink) after splitting at corners (`detect_corners` on the chain),
   fit it **once** (`fit_run`, with the bounds guard), and store the fitted
   path. A region's loops are assembled by walking its incident chains
   (oriented with the region on the right; at a junction exactly two chains
   touch any given region, so the continuation is unique), reversing a
   chain's fitted path where needed. Two regions therefore share identical
   boundary curves: no cracks, no overlaps, nothing to expand.
8. **Colour.** Per region: per-channel median of its interior pixels
   (eroded by 1 px) in the **under-painted original** (not the flattened
   picture); fewer than 8 interior pixels → the median of all its pixels.
   The paper/background label (`drop_background`) is not emitted.
9. **Output.** `TraceMode::InkAndColour`: fills first, largest area first,
   then `trace_ink`'s shapes on top; `TraceReport { regions, fills,
   message: "Traced N fills and M ink shapes" }`. Dialog: a seventh mode
   button "Ink + colour" showing Bold, Smoothness, Speck size, Colours (K).
   `trace_png` mode `inkcolour`.

### Measurement and acceptance

- `trace_png inkcolour` on Raju / Guddu / Woman1: mean ΔE to the source
  (`compute_mean_delta_e`) ≤ 3.5 on Raju, ≤ 5 on the other two; no region
  under 4 px²; **zero crack pixels**: render the fills alone at 1× and count
  pixels that are paper-coloured but lie inside the under-painted figure
  (should be 0 ± anti-alias).
- By eye at 3× (crops as in I3): skin is one tone plus its cel shadow,
  hair is one mass plus its highlights, no speck regions, no stair-steps,
  the sari border is a band with its pattern, the plaid reads as plaid, and
  the three composites read as the sources.
- Performance ≤ 3 s per character.

### Risks

- Bilateral range σ: too high flattens the sari motifs into the ground; too
  low leaves banding. Start at 10 ΔE and check Woman1's motifs survive.
- Junction handling at corner contacts (two regions touching diagonally):
  treat as `bucket::trace_contours` does (8-connected fill, 4-connected
  gaps) so the graph stays consistent on both sides.
- kurbo fit on very short chains (< 4 points): use the polyline directly.

### A2 — speck floor and colour lines (2026-10-06)

- **Thin-stripe floor.** Every blob under ~3 px² has depth ≤ 1.5, so the
  thin-stripe exemption kept every speck (Woman1 3857 fills). A stripe now
  also needs area ≥ `THIN_STRIPE_MIN_PX2` = 20.
- **Colour lines** (step 2b, between fills and ink). On the ink-under-painted
  picture: local background colour = per-channel median of 8-bit Lab over
  the ink window (three `local_background` passes); `Fc = clamp(ΔE / 20, 0,
  1)`, 0 on ink / transparent; the ink's hysteresis (core 0.7); components of
  `Fc ≥ 0.5` deeper than 2.5 px (surfaces: eye whites, teeth, bindis) or
  under `speckle` px² dropped. Line pixels are under-painted from the
  original (nearest pixel neither ink nor line) before flatten and
  segmentation, so the fill under a plaid thread is the plain cloth. Traced
  as the ink (no bold), one shape per 8-connected `U×` component, colour =
  median of core pixels in the ink-under-painted original. Output: fills,
  colour lines, ink; `TraceReport.strokes` = colour lines.
- Open: crossing threads are one 8-connected component, so a plaid traces
  as one grid shape, not one shape per thread (test (d) asks ≥ 10).

### A3 — palettes per surface, high-contrast small regions (2026-10-06)

- **Surfaces** = 4-connected components of opaque non-ink pixels
  (`surfaces`). `under_paint` also returns each painted pixel's donor; an ink
  pixel takes its donor's label (so its donor's surface), or the global
  palette's nearest when no donor reached it.
- **Per-surface palette** (`surface_palette`, ≥ `SURFACE_MIN_PX2` = 400, on
  the flattened Lab): first centre = mean of the most frequent 4-ΔE bin;
  farthest-point candidates accepted only with ≥ max(24, px/1000) surface
  pixels within ΔE 4 (rejects never re-offered; support counted over the
  27 neighbouring bins of a bin-sorted pixel list), stop at ΔE < 8 or 8
  centres; ≤ 10 Lloyd iterations; centres < `PALETTE_MERGE_DE` merged
  (`merge_centres`, shared with `palette`). Under 400 px²: one colour (Lab
  median) when its 95th-percentile ΔE (nearest rank) < 8, else the global
  palette. Labels are offset per surface (`surface_labels`).
- **Contrast exemption** in `merge_regions`: a region under
  `MIN_REGION_PX2` with area ≥ 4 and mean ≥ ΔE 20 from every 8-neighbour is
  kept.
- Test (e) `broad_plaid_bands_trace_as_strips`: every band fill ≥ 40 px²
  (strips, no blobs), but 24 band fills, not 6–14: one band set must be cut
  at every crossing, so 4 horizontal strips + 4 × 5 vertical segments is the
  geometric minimum for 4 + 4 crossing bands. Needs an architect decision
  on the bound.

### A4 — the better approach for a painterly wash (Fable, 2026-10-06)

Why A3 was not enough: a plaid painted as a wash (Guddu's shirt, median
source gradient ≈ 3.7 ΔE/px against ≈ 0.7 on skin) has NO flat decomposition
— every k-means/region cut of it is a patchwork, and merging the patches
leaves a plain shirt whose base is the wash's mean. What reproduces the
source is **base + pattern**: the ground tone as the fill, and whatever
deviates from it, where thin, as lines on top.

- **Textured surfaces** (`TEXTURED_MEDIAN_GRADIENT` 2.5): median source
  gradient over the surface. A textured surface gets at most 3 palette
  centres (`SURFACE_MAX_K_TEXTURED`); on it a region under 2 % of the
  surface (`TEXTURED_MIN_SHARE`) merges unless ≥ ΔE 30 from every neighbour
  (`TEXTURED_MERGE_MAX_DE`, also the soft-merge colour allowance between two
  textured regions). `SurfaceInfo` carries surface id / textured / area per
  pixel into `region_stats` (`textured`, `surface_area` per region).
- **Fill colour of a textured region** = `dominant_colour`: the mean of the
  pixels within RGB 14 of the densest 8-wide RGB bin (the ground); other
  regions keep the median.
- **Soft merges, chain-safe**: pairs sorted by ΔE, each judged against the
  RUNNING merged colour of the two sets (union-find with Lab sums) — without
  this sari → gold border → red blouse fused in one round. Gradient bar =
  `max(4, 1.5 × the two regions' mean interior gradient)`, capped at 7
  (`SOFT_MERGE_TEXTURE_RATIO`, `SOFT_MERGE_GRADIENT_CAP`); `SOFT_MERGE_MAX_DE`
  8 → 16.
- **Residual pattern pass** (`residual_line_coverage`): per pixel, ΔE of the
  ink-under-painted ORIGINAL to its region's fill colour, over
  `RESIDUAL_LINE_DE` 13 (0.5 at ΔE 6.5, core 0.7 at 9.1); zero within one
  pixel of another region or of ink (the seam between fills is not a line).
  Then `thin_parts_coverage`: hysteresis, and only the THIN PARTS of the
  mask survive — its morphological opening by a disc of r 2.5 px (pixels
  within r of a pixel ≥ r deep), grown 1 px, is removed. Judging whole
  components threw the plaid threads away with the broad bands they connect
  to. Specks < `speckle` dropped. Merged into the colour-line coverage
  (`max`) before `colour_line_shapes`, so threads, pleats and face lines the
  local-median detector missed appear on top of the flat base.
- `LINE_DE` 20 → 16 for the first-pass detector.
- Debug: `BUZZ_COLOUR_DEBUG_DIR=<dir>` writes `flat.png`, `palette.png`,
  `regions.png` and prints the textured surfaces.

## I5 — the character pipeline traces with Ink + colour (design, 2026-10-06)

`trace_character_whole_with` (`crates/buzz-ai/src/character.rs` ~3481) today
builds the visible picture, classifies the ink (`ink_components`, T9e),
paints under the thin ink, traces fills (`trace_options`, Outline) and ink
strokes (`ink_options`, LineArt/Brush) separately (~3644–3740), tags every
shape `(is_ink, shape)` (~3758) and sends them through the owner logic: fills
are tiled by owner share with boolean clips at the smoothed part boundary
(~3843–3905), ink is cut once at the crossing (~3906–3938), `TracedPart.ink`
(1632) is drawn after `shapes`.

Change (one implementer run, buzz-ai only):

1. `TraceCharacterOptions::ink_colour: bool` (default `true`; `false` keeps the
   T9e path exactly). With it on, replace the two traces by ONE
   `buzz_scene::trace::trace(w, h, &visible_rgba, &TraceOptions::ink_colour())`
   on the visible picture (the tracer under-paints itself; no
   `paint_under_ink`, no `protected`/`cut_paths` difference — the new ink keeps
   pupils, brows and glints whole by construction).
2. Split the report by index: `fills = shapes[..fills]`, `colour lines =
   shapes[fills..fills+strokes]`, `ink = shapes[fills+strokes..]`. Tag fills and
   colour lines `is_ink = false` (tiled like fills, in that order). An ink
   shape is tagged `is_ink = true` only when ≥ 60 % of its pixel area lies on
   `comps.line_mask` (a drawn line, cut once at part boundaries); otherwise
   (a pupil, brow, hair or shoe mass) it is tagged `false` and goes through the
   fill tiling, so T4's face logic, T11's brow parts and the lip-sync mouth
   still see masses as fills.
3. Drawing order per part stays: `shapes` (fills, then colour lines, then
   masses — keep the tracer's order within each class), then `ink`.
4. `rig_vs_source` runs unchanged; add `ink_colour` to `series_setup`'s
   options plumbing if it builds its own `TraceCharacterOptions`.

Acceptance: `cargo test --release -p buzz-ai` (lib + `trace_fidelity`,
`quality_redesign`, `trace_character`) green or with documented expectation
updates where a test asserted T9e-specific output; `rig_vs_source` on Raju /
Guddu / Woman1: ΔE not worse than the T9e numbers (1.77 / 2.60 / 7.35) and
by eye the rig sheet matches the `ink_check` composites; then rebuild the
Cast (`series_setup`) and `episode_v2`.

### I5 review — what the first rig showed and what fixed it (Fable, 2026-10-07)

Measured with `BUZZ_TRACE_INK_LOG=1` (ink-shape stats, face boxes, ink
clips), `BUZZ_TRACE_SHAPES_LOG=x0,y0,x1,y1` (every part's shapes meeting a
box) and `BUZZ_FACE_LOG=1` (eye/mouth splits), all new.

- **The outline network was tiled as a fill owned by the Head.** The
  tracer's main ink component (area 69 562 px², the whole outline with the
  hair, shoes, belt and bag fused in) had only 47 % of its area on the thin
  line mask → tagged "mass" → tiled → the Head took it whole. Rule now:
  an ink shape is INK when its line share ≥ 0.6 OR its mean width
  (`2·area/perimeter`) < `INK_NETWORK_MAX_MEAN_WIDTH` 5 px OR its bounds span
  more than `INK_NETWORK_MIN_SPAN` 0.5 of the character height (Raju's
  network: 6.6 px mean width, so the span rule catches it). Pupils (9 px)
  and brows (10 px) stay masses.
- **The pupil was clipped into two pieces at the eye-part boundary and the
  under-painted skin showed through the seam; the eye white's leaked piece
  went to the Head.** Face boxes = `protected_face_boxes` (landmark boxes)
  each grown to the segmenter's face part whose pixels centre in it (the
  landmarks stop 10 px above the drawn white's bottom), inflated 25 % + 4 px
  (`FACE_WHOLE_BOX_GROW`, `FACE_WHOLE_BOX_PAD_PX`). A shape inside goes WHOLE
  to the face part holding ≥ `FACE_PART_MIN_SHARE` 0.25 of it, else to the
  top owner. Outside the boxes a face part owns nothing (its pixels count
  for the largest other owner).
- **Collar line dashed; cream band over the belt.** Both: part A's fill ran
  under ink whose piece part B (drawn below A) owned. (a) Ink pieces are now
  `network ∩ expand_fill(boundary, INK_PIECE_OVERLAP_PX 3)`, ONE intersection
  per owner — the first version (subtract covered, union leftover) left solid
  black squares and bars to the image edge on Guddu/Woman1. (b) The colour
  tracer's under-painting stops after `UNDER_PAINT_MAX_LAYERS` 4 (on the
  1 px-grown mask): a line ≤ 6 px is painted through, a mass's core (belt,
  shoe, hair) keeps its own colour as a region. Cores ≥ `MASS_CORE_MIN_PX2`
  600 are then left out of the ink (`trace_ink_with(…, exclude)`), so the
  belt and shoes show their brown inside a dark rim; a brow or pupil stays
  one solid ink mass.
- **Eye backing painted skin, lid cut round the socket.** The Eye part's
  skin piece (L* 67) passed the L*-only white test. Eye whites now need
  chroma ≤ `EYE_WHITE_MAX_CHROMA` 20; light coloured fills are the "socket",
  drawn under the backing; `EYE_WHITE_MIN_MEAN_WIDTH_PX` 2 keeps glints out
  of the lid union.
- **Dark lens over the smile.** With no traced mouth features the Rest
  viseme was a synthetic ellipse: Rest is now empty (the drawn mouth is on
  the head). Mouth features: centre in the padded lip box and ≥ 60 % of the
  bounds inside (`is_mouth_feature`), ink included; the Mouth part's other
  shapes go to the Head under the head's ink (a collar sliver under the jaw
  line rode above it as the underlay). No hidden-completion fill for eye,
  brow or mouth parts.
- **Grey 1 px slivers at silhouette edges** (thin-stripe exemption kept the
  anti-aliased seam regions): a thin region whose mean lies within
  `SEAM_MIX_MAX_DE` 6 of the segment between two neighbours' means merges.
- Tests adjusted: `every_fill_is_the_exact_source_colour_under_it` skips
  pieces < 200 px² (boolean cut / 1 px per-region smoothing at a concave
  corner; stage B's shared edges remove it) and excludes the ink shapes'
  pixels; `dark_fills_do_not_sit_on_the_ink…` measures the ink RIM (3 px);
  `ink_coverage…` checks the light fills alone; `face_rig_symbols_and_controls`
  allows an empty Rest frame.

### I5 posing review — the Cast previews (Fable, 2026-10-07)

The rest-pose rig matched the source, but `series_setup`'s posed previews
(arms down) showed **ghosts of the T-pose sleeves** on the torso. Two causes,
both pre-existing tiling/completion rules that the colour tracer's large
single-colour regions exposed:

- **The chest's hidden completion was a whole sleeve.** T3 fills "the pixels a
  part's completed picture holds that another part covers" in the part's
  colour, behind its shapes; the segmenter's chest mask includes the sleeves,
  so the chest carried a cream sleeve-shaped patch that stayed put when the
  arm fell. Now (colour path) a part's hidden pixels must lie within
  `HIDDEN_REACH_FRACTION` 0.02 × height (8–40 px) of a pixel the part owns:
  the strip under the joint, not the neighbour.
- **"The largest owner takes the remainder" (T9b).** The rings a region
  leaves outside every simplified part boundary, and the strip the Hips mask
  took along Guddu's sleeve top, all went to the largest owner (Hips/Chest) and
  stood still. Now the remainder is split loop by loop (holes with their
  outer loop) and each goes to the qualifying owner whose BONE segment is
  nearest its centroid (`split_remainder_by_bone`). A first attempt — each
  smaller owner claims what lies within 6–8 px of its boundary — left the
  Guddu strip behind and broke `traced_shapes_stay_localised…` (a Hips piece
  8 px into the thigh); reverted.
- Test adjusted: `a_covered_part_gets_a_hidden_fill_at_index_zero…` now
  expects the reach-limited strip.

## I3 — review (Fable)

`trace_png <src> out.png ink` with `TRACE_PNG_SCALE=4` on Raju, Guddu,
Woman1; crop face / hand / torso; compare with the source at 4×. Accept
when the "What right is" paragraph holds by eye, the trace takes ≤ 1.5 s,
and shape count ≤ 60 per character. Tune `DARK_LUMA`, `MIN_CONTRAST`,
`σ₁`, the corner angle, fit accuracy from there — record each change here.

## Risks

- `kurbo::simplify` is marked experimental and does not detect corners
  itself; corners are split beforehand (D13) and the fallback (E16) covers
  a bad fit. If its curves overshoot visibly, replace it with a Schneider
  (Graphics Gems) fit on the same runs — same interface.
- Gaussian smoothing shrinks closed loops slightly (curvature flow); on 3 px
  lines with `σ₁ = 1 px` this is well under the 0.5 px bold. If measured
  shrink exceeds 0.15 px, switch D14 to Taubin (λ = 0.5, μ = −0.53).
- Dark saturated details (a red pen, a sari border) pass `f_local`; they
  are traced as ink for now. The colour stage (I4) decides them by chroma
  per component.
