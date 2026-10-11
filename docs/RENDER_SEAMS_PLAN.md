# Plan: white hairlines where colours meet (zoomed out) and at mask edges

Reported: "on zoom out I see fine white lines where two colours meet", and "the
eyes also in mask layers I see fine white lines". Builds on
`docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md` (Phases 1–4 are done).

## Diagnosis

1. **Colour seams: the renderer is being allowed to drop the seam seal.**
   Vello anti-aliases each fill on its own, so where two fills share an edge
   each covers the edge pixel by about half and the backdrop shows through
   (conflation). `SceneBuilder::fill_shape_paint_sealed`
   (`crates/buzz-render/src/lib.rs`) hides this by also stroking every opaque
   fill with a 0.9-device-pixel seal. Phase 2 of the blank-stage plan
   added `LightDetail::Unsealed`, which drops that seal when a frame is over
   budget. **Zooming out puts more artwork on screen and pushes the frame over
   budget**, so the stage steps down to `Unsealed` and every seam opens. The
   ladder's hysteresis (step up only below 40% of the budget) then keeps the
   seams after you zoom back in. Phase 4 added banded rendering, which makes
   an over-budget frame safe without giving anything up, so trading the seal
   for budget is no longer needed.
2. **Mask seams: two anti-aliased edges on the same curve.** A mask clips
   through `SceneBuilder::push_clip` (lib.rs ~1239; called from `open_mask` in
   `crates/buzz-render/src/document.rs`). The clip's edge is anti-aliased, and
   the masked artwork usually has its own edge on the same curve (an eye
   white cut to the eye shape). The two coverages multiply (0.5 × 0.5), so the
   edge pixel is a quarter covered and shows the backdrop: a white ring round
   the eye.
3. **A test broken by Phase 4** —
   `crates/buzz-app/tests/stage_lighting.rs`
   `a_frame_too_big_to_rasterise_trims_its_lighting_rather_than_vanishing`
   fails. Its unlit fixture is dense polylines (900 × 600-vertex near-circles).
   The line estimator (4a) charges **every stroke join a worst-case round arc**,
   whatever the join actually turns. So the sealed fixture reads as over
   `LINE_BUDGET` although Vello would flatten each nearly straight join to one
   line.

## Ground rules
Same as the blank-stage plan:
- No `cargo fmt`.
- Don't touch unrelated dirty files.
- Match the comment voice.
- Verify headlessly.
- Use `--release` for rendering.

## Step 1: never trade the seal for budget
- `document.rs` `LightDetail::down`: `Flat` steps to `Flat`. The automatic
  ladder stops at `Flat`, and a frame still over budget there is drawn in
  bands (Phase 4d), with every seam sealed.
- Keep the `Unsealed` variant (for `pin_detail`, and possibly a future "draft
  mode"), but nothing selects it automatically. Update the enum's doc comment
  and the `reconsider` doc.
- `bands_needed` / `over_budget` are unchanged. Check that the stage and the
  exporter enter band mode at `Flat` rather than waiting for `Unsealed`
  (grep for `Unsealed` in `stage.rs`, `app.rs` and `buzz-export/src/lib.rs`).
- Update the Phase-2 tests that expect the four-level walk
  (`the_ladder_walks_all_four_levels_and_climbs_back_one_at_a_time`,
  `unsealed_gives_up_about_half_of_flats_segments`). The first becomes "the
  ladder stops at Flat and bands take over". The second stays, but pins
  `Unsealed` explicitly.
- **Test (new, buzz-export):** a scene over budget at `Flat` exports with
  seals on. Render two abutting opaque squares of different colours on a white
  stage, inside a scene padded with enough off-edge-but-visible geometry to
  force bands. Assert no pixel along the shared edge is lighter than both
  neighbouring colours by more than 8/255 (no white seam).

## Step 2: masks don't open a hairline where the artwork meets the mask edge
- In `open_mask` (non-inverted branch), clip to the mask path **grown by half
  a device pixel**, not the exact path. Build the grown clip as the mask
  path plus its outline stroked at 1 device pixel wide, merged into one
  non-zero path with consistent winding. Reuse the existing machinery that
  already does this for silhouettes: `append_silhouette` in `document.rs`,
  `buzz_geom::outline_stroke` and `buzz_geom::turns_backwards`. The width in
  document units is `1.0 / builder.view_scale()`.
- This reveals at most half a pixel of the masked artwork beyond the mask. In
  practice the mask's outline (the eye line) is drawn above it and covers
  that, which is also why Animate's own masks show no seam.
- The inverted branch punches with the exact path, so the same half-pixel
  growth applies to its `punch` path.
- Cost: one stroke outline per mask per frame. Masks are few; confirm with
  `import_budget` that bhatta2's encode time does not move by more than 5%.
- **Test (buzz-export):** a white stage with a black mask circle whose masked
  layer is a red circle **exactly the same size**. No pixel on the ring may be
  lighter than 50% red/black coverage; the old behaviour showed a light ring.
  Compare against the same scene without the mask: the edge pixels must be
  within 16/255.

## Step 3: cost joins by the angle they actually turn
- `crates/buzz-render/src/lib.rs`, in the stroke line estimate (4a): per join,
  charge `ceil(turn / (2·acos(1 − 0.25 / max(0.25, w/2))))`, with a minimum
  of 1, where `turn` is the angle between the incoming and outgoing tangents.
  Do not charge π every time. Caps on open subpaths stay at π.
- Keep the estimate conservative. Re-run the gpu-truth calibration
  (`cargo run --release -p buzz-app --features gpu-truth --example import_budget -- <fla> 0 300 1000`
  on bhatta2, sustipur1 and road_traffic). The ratio must stay ≥ 1.0
  everywhere; report the new ratios.
- The failing `stage_lighting` test should now pass **without editing it**. If
  it still fails, report the numbers (estimate vs segments vs ceiling) and
  stop; do not change the test.

## Verification (Opus review)
- `cargo test --release -p buzz-render -p buzz-export --no-fail-fast`, plus
  `-p buzz-app --test stage_lighting --test lighting_settles --test lighting_reports --test never_hang --test stage_bands`:
  all green.
- `render_import` bhatta2 frames 0, 300 and 1000 at **0.25 scale** (zoomed out),
  compared with the same frames before, by crop. No white seams at colour
  boundaries. Eyes (bhatta2 characters at a frame where a face is on screen)
  show no light ring.
- `import_budget` warm encode times are within 10% of the Phase 4 numbers.
