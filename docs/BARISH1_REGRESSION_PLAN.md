# Plan: barish1 after the 2026-09-25 import fixes — "trees vanished, pupils vanished or distorted"

Investigated by Opus (architect), 2026-09-25 late evening. Ground truth is
Adobe Animate 2024 itself (JSFL exports, see `docs/IMPORT_FIDELITY_PLAN.md`
and the memory note on JSFL). Every claim below was measured; the pictures
are in the session scratchpad
(`C:\Users\singh\AppData\Local\Temp\claude\B--youtubeProjects-Buzzcaf-Media-BuzzAnimate\bd7c677d-48be-421f-8a2e-89b1993271b7\scratchpad\barish`).

Ground rules: no `cargo fmt`; never `git checkout --` a file with uncommitted
work (copy it aside instead); verify with headless renders, not the mouse.

## What is NOT broken (checked against Animate)

| Suspect | Result |
| --- | --- |
| Symbol geometry (eyeball, eye white, eyebrow, palm bitmap) | Bounds match Animate's `element.left/top/width/height` to 0.1 px. The grouped-shape matrix is applied **once**, which Animate confirms. |
| Duplicate-named symbols ("Duplicate Items Folder") | The stage resolves to the right copy (full library path first). |
| Symbol lengths | 4216 (body) and 4648 (face) match Animate's `frameCount`. |
| Nested graphic playhead (loop / single frame / play once, firstFrame, keyframe start, tweens, sync, rig-parented layers) | Animate follows exactly our rule: `inner = firstFrame + (parentDisplayed − keyframeStart)`, modulo length. Measured with a numbered probe symbol at every level, including inside barish1's own face and mouth. |
| The night trees | Dimmed by their `brightness −0.53` colour effect, as Animate does. At frame 0 our tree pixels are about 1.7× brighter than Animate's; the palms are black silhouettes in both. Nothing is missing in the export path. The app's stage path is unverified (see B). |
| Frame 300 of Scene 1 | 97.8 % of pixels identical to Animate's export; the differences are the eyes and mouths below. |

## What IS broken

**A. Small artwork renders wrong: dark irises swallow the eye whites, and the mouth's white teeth disappear.**
- Reproduce (no Animate needed): `render_symbol` the face alone at two sizes.
  ```
  BUZZ_NO_CAMERA=1 BUZZ_EXPORT_SCALE=1 BUZZ_PLACE=900,500,0.13 cargo run --release -p buzz-app --example render_symbol -- <barish1.fla> "ThakurBhaiFaceSide copy 2" out/small 374
  BUZZ_NO_CAMERA=1 BUZZ_EXPORT_SCALE=1 BUZZ_PLACE=900,500,1.0  cargo run --release -p buzz-app --example render_symbol -- <barish1.fla> "ThakurBhaiFaceSide copy 2" out/big 374
  ```
  At scale 1.0 (and at 0.25) the eyes have grey sclera, brown iris, black
  pupil and a glint, and the open mouth shows white teeth: identical to
  Animate. At 0.13 (the size the face has on the stage) both eyes are black
  discs with a glint and the teeth are gone. Animate's own export of the same
  body at the same size keeps sclera, brown iris and teeth
  (`ani/p15_body_s374_r0.png`, eyes at x 560–700, y 390–450).
- Bisect so far, each tried in a scratch build and reverted, none fixed it:
  `SEAM_SEAL_PX = 0`; `MASK_GROW_PX = 0`; skipping strokes narrower than one
  device pixel; `set_symbol_reuse(false)` (stamp cache); nesting depth
  (limit is 12, the eye is at 5); timing (see table above).
- Per-layer bisect of `eyesSideTemplate copy 2` at scale 0.13 vs 1.0
  (`BUZZ_ONLY_LAYERS`): the `eyeWhite` layer alone renders at mean luma 70
  where the large render downscaled gives 134; `eyeBalls` alone renders
  brighter than expected (70 vs 35) because the glint dominates. So **light
  fills come out far too dark at small sizes**, and the dark eyeball then
  covers what is left. The mouth's teeth (a white fill 2.85 units tall,
  2.6 px on stage) vanish the same way.
- Candidates left: the fill rasteriser's coverage at sub-4-px sizes
  (`buzz-render/src/lib.rs` fill path, `Fill::NonZero`, tolerance
  `TARGET_PX 0.1`), colour-space of the anti-aliasing blend (sRGB vs linear),
  `path_line_stats`/`LightDetail` budget levels dropping to `Flat`/`Unsealed`
  for a tiny shape, and the `outline_stroke` sealing of fills whose seal
  ends up wider than the fill. Bisect by rendering `eyeWhitesSumit copy 4`
  (a single light-grey shape) alone at 0.13 and checking its mean luma
  against the 1.0 render downscaled: they must agree within 10 %.

**B. Unverified: the app's stage view (not the export) may drop the trees.**
The user saw trees vanish in the app. `render_import` (export path, no
culling, no stamps) shows them. The stage path adds `default_cull`
(`buzz-app/src/stage.rs:101`) and symbol/layer stamps. Add a headless test
that opens barish1 through `App` and encodes the stage at fit zoom, and
compare the tree region (top 30 % of the frame) against the export render:
identical within 5 % of pixels.

## Steps

1. **Harness** (`crates/buzz-render/tests/small_artwork.rs`, GPU, headless,
   skipped when no GPU): a light-grey ellipse 30×8 units with a dark disc
   inside, drawn at device scales 1.0 and 0.13. Assert the small render's
   mean luma over the ellipse is within 10 % of the large render downscaled
   (LANCZOS). This fails today; it is the regression test.
2. **Bisect** with the harness: (a) fill only, no stroke, no seal; (b) add
   the seal; (c) add a 2-unit stroke; (d) inside a masked layer. Find the
   first step that breaks the luma, fix that step (do not widen or darken
   fills below one device pixel; a seal must never be wider than a quarter
   of the fill's smaller device dimension; a mask must not be grown past
   that either).
3. Re-run the face at 0.13 vs 1.0 (the command above) and compare the eye
   and mouth boxes against `ani/p15_body_s374_r0.png` (Animate) — sclera
   visible, iris brown not black, teeth white.
4. **App stage parity test** (B above), in `crates/buzz-app/tests/`.
5. Record measured numbers in the Status table.

## What the bisect found (2026-09-26)

Bisected shape by shape (`render_symbol` gained `BUZZ_ONLY_OBJECTS=i,j`;
`zz_symbol_probe` gained `PROBE_PATHS=1` for per-subpath areas). Three
causes, all in `buzz-render`, none keyed to eyes or to this file:

1. **Strokes ignored their placement's scale** (the main one). `draw_shape`
   handed `stroke.width` to the builder in document units while the path went
   through the whole placement chain, so a symbol drawn at 1/8 kept outlines
   8× too wide. The eyeball's 2-unit iris outline (Animate: `scaleMode="normal"`,
   as all 131 strokes in barish1 are) came out ~2 px on a 3.75 px iris instead
   of 0.25 px: `BUZZ_ONLY_OBJECTS=3` (the stroke alone) inked 60.9 px at scale
   0.24 where 14 are due — a constant 2 px pen at every scale. That is what
   turned irises black and ate the sclera. Fixed:
   `stroke_placement_scale(doc)` (√|det|, as `buzz_scene::stamp` already did);
   the stamp's `stroke_scale / s_q` compensation, which existed only to mimic
   the unscaled live stroke, is now 1.
   Why the earlier bisect missed it: "skip strokes narrower than one device
   pixel" skipped nothing, since no stroke was ever narrower than its
   document width.
2. **The seam seal (0.9 px) was drawn over the shape before it.** Imported
   line work is drawn first and colour regions after, so each region's seal
   ate half a pixel of line from each side: the nose and cheek lines (under a
   pixel at stage size) vanished, and the teeth were eaten by the lips. Fixed
   with **seal runs** (`SceneBuilder::begin_seal_run`): per stage layer and per
   symbol with its own artwork, the fills' seals are held and drawn *beneath*
   the run's artwork, last-drawn first, so a seal only fills the pixels its
   neighbours leave short, with the lower region's colour. A seal inside a
   mask clip / blend / alpha group stays in line as before. Stamped and live
   symbols build the same run, so the symbol cache stays exact.
   Why `SEAM_SEAL_PX = 0` did not fix it earlier: a zero width became
   `f64::MIN_POSITIVE`, which Vello strokes far heavier than its width (a 1 px
   square came out covering 2.4 px); seals under 0.05 px are now skipped.
3. **The seal grew small compact shapes** (+54 % area on a 3.75 px iris).
   Capped: `seal_width_px(thickness, extent) = min(0.9, ¼·max(2A/P,
   ¼·longest side))`, so an iris gets 0.47 px, line work 15 px long keeps the
   full seal, and anything ≥3.6 px thick or ≥14.4 px long is sealed exactly as
   before (the extent comes from bounds already in hand; the exact walk only
   for shapes under 14.4 px). The mask growth (`grow_mask_path`) shrinks in
   step.

**Cost: none measurable.** A first version cost a millisecond per frame (the
lamp-drag drop frame 10.9 → 12.0 ms, export 12.7 → 13.2 ms), almost all of it
the seal runs, not the path statistics. Now a run opens only for a layer or
symbol that draws artwork of its own **and** is under 256 device px on its
longest side (`SEAL_RUN_BELOW_PX`, from the symbol memo's resolved bounds, so
no walk). The shape extent comes from the bounds `draw_shape` already has, and
the exact area/perimeter walk runs only for a shape under 14.4 px. A/B on
prebuilt binaries, 15 interleaved runs, medians: lamp-drag drop frame
11.06 → 11.19 ms (+1.2 %), Scene 1 warm export f300 12.60 → 12.60 ms,
f374 12.50 → 12.40 ms. `dragging_a_lamp_over_a_scene_stays_interactive`
passed 3/3 in a row (10.70, 10.60, 10.96 ms).

## Status

| Item | State | Evidence |
| --- | --- | --- |
| A. Small artwork luma | Done (Opus agent, 2026-09-26) | `crates/buzz-export/tests/small_artwork.rs` — 4 tests, 3 fail on the pre-fix build, all pass now: 3.75 px eyeball in a symbol at ¼ (mean luma 178.4 vs 189.8 reference; pre-fix 154.0 vs 190.0), 2-unit stroke in a symbol at ⅛ (38.9 px of ink vs 39.3 due; pre-fix 310.7), half-pixel line under two regions (128.1 vs 124.9; pre-fix 159.3 vs 127.0), 200 px square edge pixels equal to the pre-fix build's (±1). eyeWhite layer of `eyesSideTemplate copy 2` at 0.13, frame 375, over grey, same box as the 1.0 render box-averaged: 108.6 vs 129.5 before, 127.4 vs 130.1 after (the plan's alpha-weighted metric: 70 before, 125.9 after vs 145–149 averaged down). Body `ThakurBhaiSide copy 2` at frame 374, `BUZZ_PLACE=1397,138,1`, against `ani/p15_body_s374_r0.png`, mean \|Δluma\|: face 11.8 → 9.1, left eye 31.5 → 24.3, right eye 32.8 → 17.6, mouth 42.2 → 17.5; sclera, brown iris, white teeth and the nose/cheek lines all present (picture: scratchpad `bis/final_eyes_ani_old_new.png`). `buzz-render` + `buzz-export` suites: 45 test binaries green. |
| B. App stage vs export parity (trees) | Done (Opus agent, 2026-09-26) | `crates/buzz-app/tests/stage_matches_export_barish1.rs`: `stage::build_scene` over a headless `Editor` (what `App::render` draws) vs `Exporter`, 1600×900, frames 0 and 300, fit and 2× on the top-left quarter: 0.00 % of the top-30 % pixels differ (100 % identical). The stage path does not drop the trees; skips without the FLA or a GPU. |
