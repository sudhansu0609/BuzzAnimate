# Plan: renderer follow-ups after the seams and import work

Small items left once `RENDER_SEAMS_PLAN.md` and `IMPORT_FIDELITY_PLAN.md`
landed. Ground rules as before:
- No `cargo fmt`.
- Don't touch unrelated dirty files.
- Match the comment voice.
- Verify headlessly.
- Use `--release`.

## 1. Draw rigged layers through the rig-relative interpolation (IMPORT_FIDELITY A3)

`buzz-scene` now has `LayerStack::resolved_at_rigged` and
`LayerTimeline::resolved_at_with_reference`. They interpolate a rigged child's
tween in its parent's rest frame (the layer's `tween_frame`). But the renderer
still calls plain `resolved_at`, so the rig-relative path is never used.
- In `crates/buzz-render/src/document.rs`, find every place a layer's frame is
  resolved for drawing (grep `resolved_at(`, `drawable_at`, `resolve_frame`)
  for the document's layers **and** for symbol timelines
  (`draw_symbol_contents`). Route them through the rigged variant, so that a
  layer with a `tween_frame` interpolates in that frame. Layers without one
  must produce bit-identical output.
- Check the other consumers that resolve frames for *geometry* (hit testing,
  bounds, `layer.bounds_at`, the BoundsCache and the SymbolTable
  fingerprints). Where they resolve tweened frames, switch them too, so that
  picking and culling agree with what is drawn. List each one you changed.
- **Test:** a two-layer rig built in code. The parent has a non-uniform
  rest scale (2, 1) and the child's tween rotates it 0° → 90° about a pivot.
  The drawn child's pivot at t = 0.5 matches the hand-computed
  `F · lerp(F⁻¹M0, F⁻¹M1)` to within 1e-6, measured through the render path's
  resolution (not by calling the tween directly).

## 2. Stage band mode: stop encoding the whole frame as well

When `stage_bands > 0`, `App::render` still runs the single-pass
`stage::build_scene` ladder every frame and **then** encodes each band (a
deviation Phase 4 made for safety). On bhatta2 every frame is in band mode,
so the whole frame is encoded twice.
- While `stage_bands > 0`, skip `build_scene`. The band encodes (one
  `cache.begin`/`end` around them all) do the work.
- Judge from the bands:
  - leave band mode when the largest band is under 40% of both budgets;
  - double the band count (cap 64) when any band is over.
- Move the bookkeeping `build_scene` used to do once per frame so that band
  mode still does it:
  - `self.stage_stamp`;
  - `self.stage_stale = self.lights.lights.is_stale()`;
  - `take_misses` and the `set_defer` budget.

  It runs once after all the bands, not per band.
- Before changing it, add `crates/buzz-app/examples/stage_cost.rs`. It loads a
  document into a headless `Editor` (as the app tests do), sets a 1600×900
  stage area at fit zoom, and times a warm frame at frames 0, 300 and 1000 in
  band mode, using the same calls `App::render` makes (`build_scene` and/or
  `build_band`, without the GPU submit). Report the ms/frame **before and
  after** on bhatta2. Target: after ≤ 0.6 × before.
- **Tests:** existing `stage_bands.rs` plus the lighting tests stay green. Add
  one test that steps a headless app for two frames with a document over
  budget and asserts that the second frame encoded only bands. Use a counter
  or a hook if one exists; otherwise expose a `#[cfg(test)]` counter.

## 3. `import_budget` label
In band mode the row says `WILL NOT DRAW`, which is no longer true. Print
`BANDS n` (from `bands_needed()`) when over budget, and keep `WILL NOT DRAW`
only for rows where `bands_needed()` hits its cap and a band would still be
over.

## Verification (Opus review)
- `cargo test --release -p buzz-render -p buzz-export --no-fail-fast`, plus
  `-p buzz-app --test stage_lighting --test lighting_settles --test lighting_reports --test never_hang --test stage_bands`,
  plus `-p buzz-scene`.
- The `stage_cost` before/after numbers.
- `render_import` bhatta2 frames 0, 300 and 1000 are visually unchanged
  (fewer than 50 pixels differing by more than 40/255 from the current renders,
  except inside rigged characters, where the change is intended).
