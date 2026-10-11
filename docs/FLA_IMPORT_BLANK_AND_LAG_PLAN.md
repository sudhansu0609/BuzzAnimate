# Plan: large Adobe Animate imports render blank and lag

Reported with `B:\youtubeProjects\Khayal3Baje\stories\bhatta\animate\bhatta2.fla`
(34 MB; 43 layers, 12,864 keyframes, 3,801 shapes, 12,893 instances, 589
symbols, 41 camera keys). The layers and library import correctly; the stage
shows nothing, and playback stutters.

## Diagnosis (measured, not guessed)

**The importer is not at fault.** Every instance imports and every layer draws
correctly when soloed. The frame as a whole is *dropped by the GPU rasteriser*.

1. **Vello drops frames that overflow its fixed buffers.** Vello 0.9 rasterises
   from buffers sized once with hand-picked constants (`vello_encoding`
   `BufferSizes::new`: 2^21 lines/segments/tiles, 2^23 PTCL words). Past them,
   the bump allocator fails and `render_to_texture` silently writes nothing:
   the target stays transparent (export) or keeps the last picture (stage).
   `buzz_render::document::segment_ceiling` is the proxy the code already uses
   (1,229,837 encoded segments at 960×540). This is already documented at
   `crates/buzz-render/src/document.rs:221`, but the only thing that ever gets
   trimmed is *lighting*. When the artwork alone is too big, nothing is done.

2. **The stage culls to nine times the visible area.** `crates/buzz-app/src/stage.rs:92`
   passes `builder.clip_bounds()` as the cull rect. That is the `RenderClip`
   bounds: the visible rectangle **plus 100% margin on every side**
   (`crates/buzz-geom/src/clip.rs`, `MARGIN_FACTOR = 1.0`). At fit zoom that
   culls almost nothing. The exporter, the export preview strip and thumbnails
   pass `cull: None` and cull nothing at all.

3. **Every opaque fill is encoded twice.** `SceneBuilder::fill_shape_paint_sealed`
   (`crates/buzz-render/src/lib.rs:515`) fills, then *strokes the same path* with
   a 0.9 px "seam seal". That exactly doubles encoded segments (1.59 M → 0.79 M
   on frame 0). A GPU stroke also flattens into more lines than a fill does,
   since both offset sides are flattened.

4. **The symbol stamp cache never fires on imported files.** `try_stamp_symbol`
   (`document.rs:3345`) rejects any instance whose placement is not *exactly*
   orthogonal, meaning scale 1 (`is_orthogonal(a, 1e-6)`, `document.rs:3402`). On
   bhatta2, all 33,950 rejections per frame came from this one check. 98% of them
   were uniform or near-uniform scales (for example `s0.79`) and 45% were mirrors.
   So all 12,893 instances are walked, transformed and encoded live, every frame.

The GPU choice is fine: it picks the NVIDIA RTX 5060 Ti on DX12. The lag is the
CPU encode walk (items 2–4) plus frames being dropped (item 1).

### Baseline (`import_budget` example, 960×540, warm cache)

```
frame     0 no cull           segments  1592785 (130% of ceiling, WILL NOT DRAW) detail Flat symbols built 0 stamped 0  warm 58.7 ms/frame
frame     0 stage             segments   584503 ( 48% of ceiling) detail Full symbols built 0 stamped 0  warm 22.9 ms/frame
frame     0 stage+pasteboard  segments  1048522 ( 85% of ceiling) detail Full symbols built 0 stamped 0  warm 44.0 ms/frame
frame   300 no cull           segments  1575445 (128% of ceiling, WILL NOT DRAW) detail Flat symbols built 0 stamped 0  warm 56.4 ms/frame
frame   300 stage             segments  1394900 (113% of ceiling, WILL NOT DRAW) detail Flat symbols built 0 stamped 0  warm 55.9 ms/frame
frame   300 stage+pasteboard  segments  1552160 (126% of ceiling, WILL NOT DRAW) detail Flat symbols built 0 stamped 0  warm 62.7 ms/frame
frame  1000 no cull           segments  1578029 (128% of ceiling, WILL NOT DRAW) detail Flat symbols built 0 stamped 0  warm 57.3 ms/frame
frame  1000 stage             segments   855835 ( 70% of ceiling) detail Full symbols built 0 stamped 0  warm 37.3 ms/frame
frame  1000 stage+pasteboard  segments  1250829 (102% of ceiling, WILL NOT DRAW) detail Flat symbols built 0 stamped 0  warm 51.5 ms/frame
```

Reproduce:
```
cargo build --release -p buzz-app --example import_budget --example render_import
target/release/examples/import_budget.exe "B:/youtubeProjects/Khayal3Baje/stories/bhatta/animate/bhatta2.fla" 0 300 1000
target/release/examples/render_import.exe "B:/youtubeProjects/Khayal3Baje/stories/bhatta/animate/bhatta2.fla" out.png 300
```
(`render_import` writes a fully transparent PNG today.)

## Ground rules for the implementer

- The working tree has ~150 files of **uncommitted, unrelated work**. Do not
  revert, stash, reformat or commit anything. **Never run `cargo fmt`** (the repo
  is not rustfmt-clean; a sweep buries the real diff). Touch only the files named
  below.
- Match the surrounding style: long explanatory `///` comments that say *why*,
  in the voice of the existing ones. No drive-by refactors.
- Verify headlessly (tests, examples). Never drive the real mouse or window.
- Build and run with `--release` for any timing. Vello is unusable in debug.

---

## Phase 1: cull to what can actually be seen

### 1a. Stage: cull to the visible rect, not the clip rect
- `crates/buzz-render/src/lib.rs`: `SceneBuilder` stores the camera's
  `visible_doc_rect()` given to `new` (add a field `visible: buzz_geom::Rect`),
  exposed as `pub fn visible_bounds(&self) -> buzz_geom::Rect`. Set it in `new`.
  Every struct-literal construction of `SceneBuilder` (there is one in
  `try_stamp_symbol`, `document.rs` ~3454) must set it too; use the clip rect
  there.
- `crates/buzz-app/src/stage.rs` `encode`: the cull becomes
  `builder.visible_bounds()` grown by a small safety margin of **2% of its size
  on each side** (a stroke can poke a hair outside an object's fill bounds).
  Keep the `BUZZ_NO_CULL` escape hatch. Update the comment.
- Safe because the stage's retained encoding is keyed on the view camera and
  area (`StageStamp` in `app.rs`), so any pan or zoom re-encodes anyway.

### 1b. Exporter and export preview: cull to the output
- `crates/buzz-export/src/lib.rs` `draw_into_target`: the export camera frames
  either `settings.region` or the stage rect. Pass
  `cull: Some(that rect grown by 2%)` in the `FrameOptions` used for the draw.
  Do this by cloning `options` and setting `cull` only if the caller left it
  `None`. The rect is in the camera's shot space, which is the stage's space.
  That is the same space the stage uses.
- Same in `crates/buzz-app/src/preview.rs` (~line 210) for the Export dialog's
  test strip.
- Update the doc comment on `FrameOptions::cull` (`document.rs:95`). The
  exporter now culls too, and parity holds because the cull is conservative
  (`culling_to_the_viewport_matches_no_cull`).
- **Test:** in `crates/buzz-export/tests/` (next to `headless_symbol_cache.rs`)
  add a test that renders a scene with artwork both on and far off the stage
  through `Exporter::render`, then asserts the pixels equal a render made with
  `render_with(.., FrameOptions { cull: None, lit: true, guides: false, .. })`
  with culling forced off. If a force-off switch is needed, add
  `ExportSettings`-free plumbing, for example an `Exporter::set_cull(bool)`
  defaulting to `true`.

## Phase 2: the seam seal becomes something a frame can give up

The artwork is never given up; the seal is a rasteriser workaround, and it costs
as much as the artwork.

- `document.rs` `LightDetail`: add a fourth level **`Unsealed`**, below `Flat`.
  - `down(Flat) = Unsealed`, `down(Unsealed) = Unsealed`.
  - `up(Unsealed) = Flat`.
  - New `fn seals(self) -> bool { self != Self::Unsealed }`. `models()` and
    `casts()` are both false at `Unsealed`.
  - `room_to_step_up(Unsealed)`: stepping up doubles the fills' cost, so step
    up only when `segments < ceiling * 2 / 5`.
  - Update the enum's doc comment: what is given up, and in what order. The seal
    goes **last**, after the modelling and the cast shadows.
- The three `fill_shape_paint_sealed` call sites in `document.rs` (~3873, ~4175,
  ~4241) call `fill_shape_paint` instead when `!cache.detail().seals()`. The
  `lib.rs` method stays as it is. Leave the brush-preview call at `stage.rs:245`
  alone.
- **The stamp cache key must include the seal.** A symbol encoded with seals
  and stamped at `Unsealed` would bring them back. Add `sealed: bool` to
  `SymKey` (`document.rs` ~865), set from `cache.detail().seals()`.
- The reconsider loops are bounded by the number of levels. Change `0..3` to
  `0..4` in `crates/buzz-app/src/stage.rs:64` and `crates/buzz-export/src/lib.rs:458`,
  and update the `reconsider` doc ("second pass at the latest ... three of
  them").
- `crates/buzz-app/src/app.rs:3290`: the exhaustive `match` for the lighting
  panel note needs an `Unsealed` arm, worded like the `Flat` one: "too dense to
  model or shadow — colour only". Also, `app.rs:7548` compares against `Full`;
  leave that unchanged.
- **Tests** (in `document.rs`'s test module, following
  `the_ceiling_refuses_every_frame_the_rasteriser_lost` ~5568):
  - The ladder walks `Full → NoModelling → Flat → Unsealed` under a
    persistent overload, and climbs back one step at a time once the load drops
    below each `room_to_step_up`.
  - At `Unsealed`, an opaque filled shape encodes about half the segments it
    does at `Flat` (assert `unsealed * 10 < flat * 6`).
  - Existing `crates/buzz-app/tests/stage_lighting.rs` tests that iterate
    `[NoModelling, Flat]` must still pass unchanged.

## Phase 3: stamp scaled and mirrored instances

A placement `A` whose linear part is a *similarity*, `s·R` with `R` orthogonal
(rotation, optionally mirrored), can be stamped exactly. Encode the symbol at
render scale `S·s_q` instead of `S`, and stamp with the linear part `A_lin / s_q`:

```
stamp(child(p)) = (A_lin/s_q)·(S·s_q·(p − anchor)) + S·(A·anchor − cam)
                = S·(A·p − cam)          ✓ same as the live draw
```

Stroke widths, seam widths and the flattening tolerance are then exactly what a
live draw at combined scale `S·s` would bake, up to the quantisation below.

In `try_stamp_symbol` (`document.rs:3345`):
1. From `a = proj_affine * doc`, take `det = a.a*a.d − a.b*a.c`. If
   `|det| < 1e-12`, reject. Set `s = sqrt(|det|)`.
2. **Quantise** `s` to 1/32 octave: `s_q = 2^(round(log2(s) * 32) / 32)`. Then
   `r = A_lin / s_q` has a scale within ±1.1% of 1.
3. Replace `is_orthogonal(a, 1e-6)` with an orthogonality test on `r`, with
   tolerance **`2.5e-2`**. That admits the ±1.1% quantisation residual and a
   residual anisotropy under about 1%. Genuinely stretched or skewed instances
   still draw live. Keep `is_orthogonal`'s doc accurate: it now takes a
   tolerance meant for "near-similarity after scale is factored out".
4. `let child_scale = scale * s_q;` Use it for the `scale * diag > 1e5` guard,
   for the child `RenderSplit { scale: child_scale, .. }`, and for the key
   (`scale_bits: child_scale.to_bits()`). The stamp's translation keeps the
   **stage** `scale`.
5. `symbol_stamp(...)` gets the linear part `r` instead of `a`'s:
   `gpu_view * Affine::new([r0, r1, r2, r3, t.x, t.y])`, with `t` still
   `((a * anchor) - cam) * scale`. `stamp_scene` needs `s_q` passed through, or
   the ready-made stamp affine. Update `stamp_matches_the_live_split_formula`
   (~5492) to cover a scaled and a mirrored placement.
6. **Do not stamp a mostly off-screen instance.** Stamped children are encoded
   without culling (`cull: None` in the neutral ctx), while the live path culls
   the parts of a huge background symbol that lie off-screen. If `ctx.cull` is
   `Some(c)` and the instance's shot-space bounds (resolved bounds through
   `a`, then `ctx.projection.map_rect_bounds`) have **less than half their area
   inside `c`**, return `false` and let the live, culled path draw it.
7. `SYM_CACHE_CAP` (~892): 256 is far too few for 589 symbols × scale buckets ×
   animated `inner` frames, and it thrashes. Raise it to **4096**.
- **Tests:**
  - `n_instances_build_once` (~5539): add a variant where the instances are
    placed at scale 0.79, and mirrored at `(-0.79, 0.79)`. They must build
    once and stamp N−1 times.
  - A pixel-parity test in `crates/buzz-export/tests/headless_symbol_cache.rs`
    style: a symbol with fills **and** a stroke, placed at scales 0.5, 0.79, 1.7
    and mirrored. Render with `set_symbol_reuse(true)` and with `false`, and
    assert every channel is within **2/255**, with at most 0.5% of pixels
    differing at all (AA noise from the quantised scale). *(Relaxed after measurement to <= 24/255 and >= 98.5% identical; see "Status after Phases 1-3".)*
  - `crates/buzz-render/tests/encode_cost.rs` must still pass.

## Status after Phases 1–3 (verified 2026-09-23)

Done and verified independently: all nine `import_budget` rows are under the
segment ceiling, warm encode is 3.8–9.6 ms/frame (was 23–63), and stamping
engages (hundreds built, thousands stamped per frame). The tests pass. Two
deviations were accepted:

- **Stroke width under a stamp.** The live draw never scales `stroke.width` by
  the instance's scale, so a stamped child carries `DrawCtx::stroke_scale = 1/s_q`
  (keyed in `SymKey`). The scaled-stamp parity gate in `symbol_stamp_scale.rs`
  is ≤ 24/255 with ≥ 98.5% of pixels identical. The residual (17/255 on single
  stroke-edge pixels) stays at 15 even with quantisation switched off, so it is
  Vello stroking under the append transform, not a width error; before the fix
  it was 223. The unscaled suite (`headless_symbol_cache.rs`) keeps ≤ 2.
- `Exporter::set_cull(bool)`, default on, exists so a parity test can still ask
  for a genuinely unculled render.

**But the segment ceiling is not a safe trigger.** `render_import` at frame 1000
still exports a fully transparent PNG. Vello's real GPU counters, read through
its async path with the `debug_layers` feature, show:

| frame / cull | encoded segments | % of segment ceiling | **actual lines** | % of 2^21 line buffer |
|---|---|---|---|---|
| 0, stage, detail Full | 694,273 | 56% | 1,821,697 | 87% — drawn, barely |
| 0, stage+pasteboard, Unsealed | 623,304 | 51% | 689,701 | 33% |
| 1000, stage, detail Full | 1,040,574 | **85%** | **≈2,695,007** | **128% — dropped** |

`segment_ceiling` assumes about 1.7 lines per encoded segment. This artwork runs
at about 2.6 (curvy fills, and strokes that flatten on both sides plus their
joins), so the ceiling passes a frame the GPU then drops. Vello's own
`bump_estimate` feature is no substitute: it reported 143–379% on frames that
draw fine, and its `tile` and `ptcl` fields are unimplemented (always 1). The
binding constraint is the **line buffer**: tiles peak at 15% and PTCL at 6%.

## Phase 4: never drop a frame (hard guarantee)

### 4a. Estimate lines on the CPU, the way Vello flattens them
`SceneBuilder` (`crates/buzz-render/src/lib.rs`) keeps `lines: u64`, and adds to
it at **every** point where it hands Vello geometry: the 5 `self.scene.fill`, the
3 `self.scene.stroke` and the 6 `self.scene.push_layer` (clip paths) calls, plus
`stamp_scene`'s `builder.scene.append` in `document.rs` (~3705). Expose it as
`pub fn estimated_lines(&self) -> u64`. Every estimate is taken on the path **in
device pixels**, which is what Vello flattens in: the render-space path through
`split.gpu_view`'s scale, or through the path's own transform for
`stroke_transformed`.

Mirror `vello_shaders` `flatten.wgsl` (tolerance **0.25 device px**):
- **Line segment:** 1.
- **Quad / cubic:** Wang's bound, which is always ≥ Vello's Euler-spiral count:
  `n = max(1, ceil(sqrt(0.75 · M / 0.25)))`, where
  `M = max(|p0 − 2p1 + p2|, |p1 − 2p2 + p3|)` for a cubic. Elevate a quad to
  cubic first. Cap at 100 per segment, as Vello does (`clamp(…, 1, 100)`).
- **Fill:** the sum over the path, plus 1 per closed subpath (the closing line).
- **Stroke of width `w` px:** `2 ×` the fill count, since both offset sides are
  flattened, plus per join, and per cap for open subpaths, the round-arc count
  `ceil(π / (2·acos(1 − 0.25 / max(0.25, w/2))))`, using the worst case
  (`round`) whatever the join style is.
- **`push_layer` clip:** the fill count of the clip path.
- **Stamp:** the child's own `estimated_lines()`, stored in `SymEntry` when the
  child is built (the child is already encoded at its final device scale, so
  there is no rescaling). A hit adds the stored number.

Make it cheap: add per segment inside the walk that `to_render_space` already
does, rather than walking the path again.

**Unit tests** in `lib.rs`: a 100 px circle's fill estimate lies within
[1.0×, 2.0×] of `ceil(2π / (2·acos(1 − 0.25/100)))`; a straight-edged
rectangle is exactly 4 (+1); a stroke is ≥ 2× its fill.

### 4b. Ground truth, opt-in, for calibration
- `crates/buzz-render/Cargo.toml`: feature `gpu-truth = ["vello/debug_layers"]`.
  `crates/buzz-app/Cargo.toml`: feature `gpu-truth = ["buzz-render/gpu-truth"]`.
  Neither is a default feature, and a normal build must not change.
- `#[cfg(feature = "gpu-truth")] pub fn GpuContext::render_measured(&mut self, scene, target, w, h, base) -> Result<GpuTruth>`
  calls the deprecated `render_to_texture_async` with
  `vello::low_level::DebugLayers::none()`, and polls the device on a helper
  thread (`device.poll(PollType::Poll)` in a 1 ms loop) **or the map never
  completes and it hangs forever**. `GpuTruth` holds `lines`, `segments`,
  `ptcl`, `tile` and `failed`.
  **Overflow panics inside Vello** (`lib.rs:803`, "slice offset 0 size N is out
  of range for buffer of size 50331648"). Wrap the call in
  `std::panic::catch_unwind(AssertUnwindSafe(...))` and report a panic as
  `GpuTruth { overflowed: true, lines: N / 24 parsed from the message if
  possible, .. }`. Rebuild the `GpuContext` after a caught panic.
- `crates/buzz-app/examples/import_budget.rs`: under
  `#[cfg(feature = "gpu-truth")]`, after each row, print the GPU truth next to
  `estimated_lines()`, as `lines est X / actual Y (ratio R)`. Run with
  `cargo run --release -p buzz-app --features gpu-truth --example import_budget -- <fla> 0 300 1000`.
- **Calibration target:** on bhatta2 (frames 0, 300, 1000; all three culls) and
  on `sustipur1.fla` and `road_traffic.fla` (frame 0 and one mid frame), the
  estimate is **never below the actual** (ratio ≥ 1.0), and on average **≤ 1.6×**
  it. If Wang proves too loose, fit a single multiplier and document the data,
  but never let any sample fall under 1.0.

### 4c. The ladder judges lines as well as segments
- `DrawCache` gets `lines: u64` and `pub fn note_lines(&mut self, lines: u64)`,
  called by every `reconsider` caller just before `reconsider`
  (`stage.rs` ~64, `buzz-export/src/lib.rs` ~495). `reconsider` treats the
  frame as over budget when **either** `segments > ceiling` **or**
  `lines > LINE_BUDGET`, where `const LINE_BUDGET: u64 = 1_800_000` (86% of
  2^21). Its step-up rule likewise requires both measures to be under their
  `room_to_step_up` fraction. A caller that never calls `note_lines` gets
  exactly today's behaviour (lines = 0), so the existing segment-based tests
  stand.
- `pub fn over_budget(&self) -> bool` (either measure over), and
  `pub fn bands_needed(&self) -> u32` =
  `clamp(ceil(1.3 · max(encode/ceiling, lines/LINE_BUDGET)), 2, 16)`.
- `app.rs` HUD/panel: nothing new required.

### 4d. Draw in bands when still over budget at `Unsealed`
Each band is culled to its own strip, so it encodes a fraction of the geometry
and gets its own Vello pass.

**Exporter** (`draw_into_target`, after the reconsider loop), if
`self.lights.over_budget()`:
- Split the output height into `n = bands_needed()` equal strips. For each,
  cull to its document-space rect (the export camera's framing sliced to those
  rows, grown by 2%), encode on the **full** export camera **without
  reconsidering** (the level is held for the bands), render into the target,
  `read_raw()`, and copy that strip's rows into a CPU RGBA buffer.
- If any band's own `estimated_lines()` is still over `LINE_BUDGET`, redo the
  whole frame with `2n` bands (cap 64).
- Upload the stitched buffer into the target exactly as `render_blurred` does
  (`queue.write_texture`), then carry on to the compositor.

**Stage** (`app.rs`, around `stage::build_scene` ~7651 and `gpu.render` ~7860):
- `self.stage_bands: u32` (0 means a single pass). After `build_scene`, if
  `self.lights.over_budget()`, set `stage_bands = bands_needed()`.
- While `stage_bands > 0`: refactor `stage::encode` to take the cull as a
  parameter, and add `stage::build_band(vello, editor, area, scale, cache, band_px: Rect) -> u64`.
  It uses the band's document rect (the view camera's inverse over those
  physical rows ∩ `area`, grown by 2%) as the cull, does no `reconsider`, and
  returns `estimated_lines()`. Band `i` covers target rows `[h·i/n, h·(i+1)/n)`.
  Render it into a **scratch** texture of the target's size and format (usage
  `STORAGE_BINDING | COPY_SRC`, sized like `ensure_target`), then copy that
  band's rows into the target in its own encoder and submit before the next
  band. Add `COPY_DST` to the target's usage (`ensure_target` ~1133). Wrap all
  the bands in one `cache.begin()`/`cache.end()`.
- Leave band mode when the largest band is under 40% of both budgets
  (`stage_bands = 0`; the next frame runs the normal ladder). If any band is
  over, double `stage_bands` (cap 64) for the next frame.
- The retained-encoding `reuse` must be false while `stage_bands > 0`.

### 4e. Tests
- The exporter test from the original plan: a synthetic scene over budget at
  512×512 (assert the precondition `over_budget()`) exports with > 95% of pixels
  non-transparent.
- A headless `crates/buzz-app` test: `stage::build_band` over the top and
  bottom halves returns fewer lines each than the whole-area encode, for a
  scene with artwork spread vertically.
- Every existing test from Phases 1–3 stays green.

## Phase 5: verification (acceptance)

Run everything with `--release` where timing matters.

1. `cargo test -p buzz-geom -p buzz-render -p buzz-export`, plus
   `cargo test -p buzz-app --test stage_lighting --test lighting_settles --test lighting_reports --test never_hang`:
   all green. Report any pre-existing failure separately, having shown it
   fails the same way with your changes stashed out. (Only do that stash
   check with the user's permission; otherwise just report it.)
2. `import_budget` on bhatta2, frames 0 300 1000:
   - **no row says `WILL NOT DRAW`** for the `stage` and `stage+pasteboard`
     rows;
   - `symbols built` > 0 and `stamped` ≫ `built`;
   - `warm` ≤ **25 ms/frame** on the `stage` and `stage+pasteboard` rows
     (baseline 23–63).
3. `render_import` on bhatta2 at frames 0, 300 and 1000 writes PNGs that are
   > 95% opaque and not a single flat colour. Look at them. (After Phases 1-3, frames 0 and 300 pass and **frame 1000 is still blank**: that is the acceptance case for Phase 4.)
3b. With `--features gpu-truth`, no `import_budget` row reports `overflowed`, and the estimate/actual ratio meets the 4b calibration target.
4. The same two examples on two other real files:
   `B:/youtubeProjects/extra/poorane din/sustipur1/Animate/sustipur1.fla` and
   one `B:/youtubeProjects/Khayal3Baje/Effects/Assets/*/road_traffic.fla`. None
   blank, none slower than before.

## Out of scope (note, do not do)
- Enlarging Vello's buffers by vendoring `vello_encoding`. That only adds
  headroom, not a guarantee, and pins a fork.
- Stamping under active lighting (`ctx.lighting.is_some()` still refuses).
- Missing bitmap `coconutTree.png` and the two open fill outlines reported by
  the importer. These are separate fidelity issues and do not cause blankness.
