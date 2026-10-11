# Plan: Animate imports at ≤ 20–25 ms per frame, with no lag

Requested 2026-09-24. **The AI plan is paused** (`LOCAL_AI_PLAN.md`) once its
metrics-fix run lands. Test files:
- `B:/youtubeProjects/Khayal3Baje/stories/bhatta2/animate/2Bhatta2.fla`
  (31 MB: 28 layers, 9,640 keyframes, 3,062 shapes, 10,100 instances, 436
  symbols, 34 camera keys);
- `B:/youtubeProjects/Khayal3Baje/stories/bhatta2/animate/RECOVER_2Bhatta1.fla`
  (73 MB: 35 layers, 11,288 keyframes, 8,144 shapes, 11,423 instances,
  1,141 symbols, 40 camera keys).

Target: **every** frame of both files costs ≤ 25 ms (≤ 20 ms preferred)
end to end during playback and scrubbing in the editor at 1600×900. That's
CPU encode plus GPU raster plus present, not CPU encode alone. The idle
editor (retained encoding) must stay about 0 ms.

## Status
| Step | State | Date | Evidence |
|---|---|---|---|
| Baseline | measured (CPU encode only, 4 frames) | 2026-09-24 | the table below |
| **H0 hotfix: black half-screen and 600 ms frames** | done: correctness fixed, speed much worse | 2026-09-25 | 0 of 354 bands blank. But 2Bhatta2 frames 1700–2000 take **10–13 s** (up to 55 bands), and RECOVER peaks at 616 ms. The old fast numbers were fast because bands rendered **blank**. A path crossing many bands costs its full lines in every band, so splitting can't get under budget, and splitting re-encodes up to 127 times. **Conclusion: P1 (bigger buffers) is required; bands are only a last resort.** |
| **P1 bigger Vello buffers** | done, verified | 2026-09-25 | scale 8×, 1.29 GB transient; 0 bands and 0 blank on every sampled frame of both files; gpu-truth min ratio 1.05, no overflow; bhatta2 renders pixel-identical at 0/300/1000. 2Bhatta2 p95 26.1 ms. Opus re-run max **90.6 ms**: frames 1900/1925 at 64–76 ms are first-appearance (cold stamp) spikes, not the heaviest frames. RECOVER p95 22.9, max 32.6. One flaky test: the process-wide buffer scale races between tests |
| **P2 per-layer cache + stamp prewarm + per-cache budget** | **FAILED review, cache OFF by default** | 2026-09-25 | User report: import looks low quality, lines fat, objects vanish at high zoom. Opus A/B on 2Bhatta2 (`BUZZ_LAYER_CACHE=1` vs off): frame 300 export 5.6% of pixels wrong, worst 255 (wheel spokes filled solid, outlines fattened, kaash swollen); editor ×16 view zoom 3–32% identical, i.e. **artwork gone, only the background left**. Cache off matches Animate's own `bhatt2Final.mp4` close-up. `LayerSceneCache` now defaults to off (`BUZZ_LAYER_CACHE=1` opts in); buzz-render tests pass. Speed with the cache off (`stage_cost`, 2Bhatta2, 178 frames): median 6.7 ms, p95 25.5, max 28.1, consecutive 1880–1960 steady 21.3 ms, 0 bands, 0 blank. That is no worse than P1 (p95 26.1, max 90.6), so the cache bought little. Frame 1719 (mid camera tween) was also wrong with it on: 30% of pixels, scene shrunk into black borders. Must not be re-enabled until P2's own parity gate passes at 1×, 4×, 16×, 40× view zoom and during camera moves. |
| P0 profiler sweep | not started | – | – |
| P3 parallel band encode (if still needed) | not started | – | – |
| P4 verify both files, every frame | not started | – | – |

## Baseline (`stage_cost` example, 1600×900, warm, CPU encode only)
| File | Import | Frame 0 | 300 | 1000 | 2000 |
|---|---|---|---|---|---|
| 2Bhatta2 | 705 ms | **85 ms**, 5 bands | under budget (single pass, not timed) | under budget (single pass, not timed) | **42 ms**, 3 bands |
| RECOVER_2Bhatta1 | 1551 ms | under budget (not timed) | 12 ms, 2 bands | under budget (not timed) | 12 ms, 2 bands |

Known costs:
- Each band re-walks the scene and is its own Vello pass. Bands exist
  because Vello 0.9's line buffer is fixed at 2^21 lines
  (`docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`).
- Nothing is cached per layer between frames, except symbol stamps, so a
  static background is re-encoded every frame.

## H0 — hotfix (the user reports: ~600 ms frames, half the screen black on import)
**Diagnosis (Opus, from code).** `render_stage_bands` (app.rs ~1318):
- In band mode the symbol-stamp rule is judged against the **full view**
  (`stamp_view`, added in the follow-ups run), so a stamped character is
  appended **whole into every band it touches**. More bands therefore don't
  reduce the largest band's lines.
- `still_over` then doubles `n` every frame, up to 64, and 64 full-window
  Vello renders make the ~600 ms frames.
- A band still over the GPU line buffer rasterises **nothing**, so its rows
  are copied into the target **black**.

**Fix:**
1. In band mode, the stamp rule uses the **band's** cull rect again: stamp
   only when the instance is ≥ 50% inside the band, otherwise draw it live
   and culled. `stamp_view` stays for single-pass frames (it's harmless
   there). Apply the same in the exporter's band path.
2. **No blind doubling.**
   - After rendering the bands, find each band whose own estimate is still
     over budget and **split just that band** in two, recursively (up to 6
     levels), before rendering it. The band is not rendered at all while it
     is over budget.
   - If a band can't get under budget at the deepest split, render it at
     `LightDetail::Unsealed` (pinned for that band only).
   - If it's still over after that, render it anyway, and log a warning with
     the numbers.
3. `n` for the next frame is the number of leaf bands actually used this
   frame, capped at 16, not doubled every frame. Leave band mode as now.
4. Log each frame's band count and ms under `RUST_LOG=buzz_app=debug`.
5. **Test (headless):** simulate `render_stage_bands`' planning with a
   heavy-instance scene. Every band's estimate ends up ≤ `LINE_BUDGET`,
   the band count stays ≤ 16, and a stamped instance spanning two bands is
   drawn live in both, not stamped whole into both.
6. **Measure:** extend `stage_cost` to run the exact app band planner,
   including the GPU renders into an offscreen target, and read each band
   back to check it isn't blank where the full render has pixels. Sweep
   every 25th frame of both test files, and report the max and p95 ms, the
   number of blank bands (must be 0), and the max band count.

## P0 — measure properly first
Extend `crates/buzz-app/examples/stage_cost.rs` to sweep **every 10th frame
of the whole timeline**. Per frame, report:
- bands;
- the time for walk/encode, stamp appends, and GPU raster (render to an
  offscreen target via `GpuContext::render` and wait on the queue, which
  gives honest GPU time);
- total ms.

Print the p50, p95 and max, plus the 10 worst frames. Run it on both files.
This is the baseline every later step is judged against.

## P1 — bigger Vello buffers, so most frames are a single pass (**priority after H0**)
Additions after H0:
- Cap the recursive band split at **2 levels** (≤ 4 leaves per top band,
  ≤ 16 in total). A leaf still over budget renders at Unsealed, then renders
  anyway. With the bigger buffers that's safe in practice; the gpu-truth
  calibration must confirm it.
- The sweep reports each frame's single-pass line estimate as well, so the
  frames that need bands are visible.
- `[patch.crates-io] vello_encoding = { path = "third_party/vello_encoding" }`:
  a verbatim copy of vello_encoding 0.9.0 with one change. `BufferSizes::new`
  reads its `lines/segments/seg_counts/tiles/ptcl/bin_data` sizes from a
  process-wide `AtomicU32` scale (default 1, the upstream sizes), set before
  the renderer is created.
- `GpuContext::new`: request `required_limits` from the adapter
  (`max_storage_buffer_binding_size` and `max_buffer_size` at the adapter's
  own limit), and pick the largest scale in {4, 2, 1} whose buffers fit those
  limits and at most 1 GB of total transient memory. Log it.
- `LINE_BUDGET` and `segment_ceiling` scale with it (read them from one
  accessor, not a constant).
- Keep bands as the guarantee for frames beyond even the bigger budget.
- **Verify:**
  - the gpu-truth calibration (`import_budget --features gpu-truth`) still
    shows estimate ≥ actual;
  - no frame is blank;
  - P0's sweep shows the band count and time dropping;
  - bhatta2 renders are pixel-identical (`render_import` 0/300/1000 against
    the current renders).

## P2 — per-layer encode cache (static layers cost nothing), plus prewarm and a per-cache budget
Additions after the P1 review:
- **Stamp prewarm:** after import, and when the playhead jumps (scrub or
  seek), build the symbol stamps for the next ~2 s of frames on a background
  job (`buzz-jobs`), in the view scale's quantised buckets, so a new shot
  doesn't pay the cold encode on its first frame. The target is that no
  first-appearance frame costs more than 1.3 × the median.
- **The budget belongs to `DrawCache`, not the process.** `DrawCache` stores
  its own line budget and ceiling scale, set from the `GpuContext` it serves.
  `DrawCache::default()` stays at scale 1, so tests are deterministic. The
  global Vello buffer scale only ever **rises**; never lower it at runtime.
  Remove the `set_buffer_scale(1)` workarounds from the tests, and make
  `stage_lighting` deterministic under the parallel run (run it 3 times).
- The sweep also runs a **consecutive** stretch (frames 1880–1960, every
  frame), which is what real playback does, with steady-state and
  first-frame numbers reported separately.
- In `crates/buzz-render/src/document.rs`, keep a cache of encoded
  `vello::Scene` fragments per layer, keyed by:
  - the layer's resolved content identity at this frame (the `Arc` pointers
    of the resolved objects, or a tween-state hash);
  - the lighting and detail level;
  - the view scale bits;
  - the camera transform bits;
  - the cull rect, quantised to a 64-px grid of the view, with the cache
    built for a rect 25% larger so small pans still hit.
- On a hit, append the cached fragment instead of walking the layer.
  Invalidate with the document revision and the existing generation
  eviction; cap the memory at 256 MB.
- The document camera animates on these files (34–40 camera keys), which
  would miss every frame. So encode layer fragments in **pre-camera** space
  and apply the camera as the append transform whenever the camera is a
  similarity (no tilt or projection). That's the same trick symbol stamps
  already use (`symbol_stamp`).
- **Verify:**
  - pixel parity against uncached renders (≤ 2/255 per channel, ≥ 99.9%
    identical) on bhatta2 and both test files at 5 frames each, including
    frames mid camera move;
  - the P0 sweep before and after.

## P3 — only if P1 and P2 leave frames over 25 ms: encode bands in parallel
Give each band its own `vello::Scene` and encode the bands on rayon threads.
`DrawCache` is shared read-mostly: take cache misses under a mutex, or
pre-build symbol stamps in a first pass. Submit the passes in order.

## P4 — acceptance
- The P0 sweep on both files: **max ≤ 25 ms, p95 ≤ 20 ms**, CPU + GPU.
- The import time is reported (≤ 2 s is expected).
- Editor interaction stays smooth: moving a selected object on a heavy frame
  re-encodes only its layer (P2).
- Tests: `-p buzz-render -p buzz-export --no-fail-fast`, plus the buzz-app
  lighting, stage_bands and never_hang tests.
- Opus reviews every number and compares renders.

## Rules
- One implementer at a time; lean briefs.
- Update this Status table after every run and every review.
- No `cargo fmt`, no commits, don't touch unrelated dirty files.
