# Adaptive Vello buffer scale: stop reserving 1.3 GB of VRAM for a blank stage (2026-09-27)

**Status: V1–V5 implemented (2026-09-27)** — was planned (user: "ignore for now but create a plan file
to implement it"). Blank-doc transient buffers measured 1302 MB → 175 MB
allocated after the first frame (see Measurements below);
`cargo test -p buzz-app --lib -- stays_interactive` could not be re-verified
this pass — `buzz-app`'s own `#[cfg(test)]` build is currently broken by
extensive, unrelated, pre-existing work-in-progress (a non-exhaustive
`buzz_ui::Command` match and several wrong-arity calls in `editor.rs`, none
of it touched by this change) — see notes.

## The report

BuzzAnimate uses about 2 GB of VRAM on a freshly opened blank document (RTX
5060 Ti, 16 GB).

## Root cause (Opus, verified in code and the startup log)

- `crates/buzz-render/src/lib.rs` `GpuContext::new` calls `pick_buffer_scale`,
  which picks the **largest** of `{8, 4, 2, 1}` that fits the adapter's
  binding limits and `TRANSIENT_BUDGET_BYTES` (1500 MB), then calls
  `vello_encoding::set_buffer_scale(scale)` once, process-wide. It came from
  P1 in `docs/IMPORT_PERF_PLAN.md`: heavy FLA imports fit one Vello pass instead
  of falling back to bands.
- `third_party/vello_encoding/src/config.rs` `BufferSizes::new` multiplies the
  six bump buffers (`lines`, `segments`, `seg_counts`, `tiles`: `1<<21 ×
  scale`; `bin_data`: `1<<18 × scale`; `ptcl`: `1<<23 × scale`) by that scale
  **on every render**, whatever the scene holds.
- The startup log confirms it: `Vello transient buffer scale (P1) scale=8
  transient_mb=1288`. Those buffers are allocated on the first frame drawn,
  a blank stage included.
- The rest of the ~2 GB: the swapchain, the stage's render targets, egui
  textures, and driver headroom from `MemoryHints::Performance`.
- `set_buffer_scale` **only rises** (`fetch_max`), by design (see the doc on
  `BUFFER_SCALE`): the `DrawCache` captures the scale once (P2) and judges
  frames against it.

## Goal

A blank or typical document uses Vello's upstream buffer sizes (scale 1,
~160 MB of transient buffers). The scale steps up **only when a frame actually
needs it**, so heavy imports keep single-pass drawing. There must be **no
quality loss**: the user's rule is quality over speed on import (see memory
`quality-over-speed-import`). Stepping up must happen *before* the detail
ladder degrades a frame, not after.

## Design

1. **Start at 1.** `GpuContext::new` computes `max_scale` (today's
   `pick_buffer_scale` result) but sets the live scale to 1. Store both on
   `GpuContext` (`buffer_scale: Cell/AtomicU32`, `max_buffer_scale`).
2. **A per-context scale, not a process-wide one.** Replace the global
   `BUFFER_SCALE` read in `BufferSizes::new` with a scale passed in
   (`RenderParams`/`RenderConfig` carrying it), so two contexts (window +
   exporter + thumbnails) can differ and the value can **fall** again. Keep
   `set_buffer_scale` as a deprecated shim for tests until callers move over.
   If threading it through the patched `vello_encoding` is too invasive, the
   fallback is keeping the global but allowing it to fall, gated by a
   generation counter the `DrawCache` checks each frame.
3. **Step up on demand.** `DrawCache::reconsider` already knows when a frame
   overflows (`segments > ceiling || lines > budget`). New order:
   *overflow → if `scale < max_scale`, request scale × 2 and re-encode the
   same frame at the **same** detail level*; only when already at
   `max_scale` does it step the detail ladder down as today. The request goes
   to the `GpuContext` owning this cache (return a `Reconsider::GrowBuffers`
   variant rather than a bool).
4. **Step down lazily.** After N seconds (e.g. 20 s) of frames that would fit
   at half the scale (headroom check `< 40 %` of the half-scale ceiling),
   halve the scale and **drop Vello's pooled buffers** so the memory is
   actually returned (Vello's `ResourcePool` keeps freed buffers for reuse, so
   stepping down without clearing the pool frees nothing; rebuild the
   `Renderer` or clear its pool). Never step down during a gesture or
   playback.
5. **Export stays generous.** `export_service` / headless rendering create
   their own context: start those at `max_scale` (an offline render has no
   frame budget and should never degrade). Only the interactive window adapts.
6. **Optional memory hint.** Try `MemoryHints::MemoryUsage` for the window
   device if it does not hurt frame time; measure before keeping it.

## Stages

| # | Stage | Status |
|---|---|---|
| V1 | DONE — `crates/buzz-render/examples/vram_probe.rs` (new): `GpuContext::new`, a blank `vello::Scene`, `wgpu::Device::generate_allocator_report()` before/after (DXGI not used — wgpu's own report was available and simpler; no new dependency). Baseline measured below; the heaviest-FLA case is exercised (not separately VRAM-measured) by `stage_matches_export_barish1.rs`'s existing pixel-parity test against `barish1.fla` |
| V2 | DONE — `crates/buzz-render/src/lib.rs` (`GpuContext::max_buffer_scale` + live, mutable `buffer_scale` starting at `1`; `grow_to`/`start_generous`/`shrink_to`), `third_party/vello_encoding/src/{config.rs,lib.rs}` (`BUFFER_SCALE` is now **thread-local**, not process-wide — `RenderConfig::new`'s call site lives in the unforked `vello` crate, so a per-context field could not be threaded all the way through without forking `vello` itself; thread-local removes the race between concurrent contexts the old process-wide `fetch_max` only made harmless, without a fork); `crates/buzz-export/src/lib.rs` (`Exporter::new` calls `start_generous()` — numerically a no-op vs. the old behaviour, confirmed by disabling it and getting byte-identical output); `crates/buzz-app/src/app.rs` (window adopts `max_buffer_scale`; `render_stage_bands`/`split_stage_band` moved off the thread-local free functions onto `DrawCache::segment_ceiling`/`line_budget` — the one place V2's audit found still reading the global for production ceiling math); `crates/buzz-app/tests/stage_matches_export_barish1.rs` (`ctx.start_generous()`, the one test found still assuming a window `GpuContext` starts at max) |
| V3 | DONE — `crates/buzz-render/src/document.rs` `DrawCache::reconsider` (grows `scale` towards `max_scale` before touching `LightDetail`; new `max_scale`/`set_max_scale`/`scale` accessors, default `1` so every caller that never raises it — the whole existing test suite — is unaffected); tests `an_overflow_grows_the_buffer_scale_before_it_trims_detail` and `once_at_max_scale_the_ladder_takes_over_exactly_as_before` |
| V4 | DONE — lazy step-down: `DrawCache::note_shrink_headroom`/`pending_shrink`/`commit_shrink` (20 s idle at <40% of the half-scale budget); `GpuContext::shrink_to` rebuilds the whole `Renderer` (no vello fork needed — `build_renderer` was already the one choke point `render_measured`'s panic-recovery path reused) so Vello's pool is actually dropped; wired into `crates/buzz-app/src/app.rs`'s frame loop gated on `!active.dragging && !self.editor.playback.playing`. Tests: `shrinking_waits_out_the_idle_window_then_offers_half_the_scale`, `an_uncomfortable_frame_cancels_a_pending_shrink` (document.rs, no GPU), and `crates/buzz-render/tests/buffer_scale_adaptive.rs` (`shrink_to_lowers_the_scale_and_rendering_still_works`, `shrink_to_gives_transient_memory_back_where_the_backend_can_report_it` — measured 1293 MB → 166 MB allocated after a poll) |
| V5 | DONE (see measurements below); `stays_interactive` perf tests could not be re-verified this pass — see notes |

## Risks

- **A one-frame hitch** when the scale grows (buffer reallocation plus a
  re-encode). It's acceptable once per session per level; measure it in V5.
- **Tests or code that assume the process-wide scale only rises** (P2
  comments in `document.rs` ~260/~463/~508, `DrawCache::scale`). Audit them all
  in V2.
- **Several GPU contexts in one process** (window, exporter, thumbnails) are
  the reason the global is only-rises today. The per-context scale removes the
  race rather than working around it.

## Files

`crates/buzz-render/src/lib.rs` (GpuContext), `crates/buzz-render/src/document.rs`
(DrawCache::reconsider, scale capture), `third_party/vello_encoding/src/config.rs`
(BufferSizes), the Vello renderer's resource pool (the `vello` 0.9 crate; check
whether its pool can be cleared without a fork), `crates/buzz-app/src/export_service.rs`
and `headless.rs` (start export contexts at max).

## Measurements (V1/V5, RTX 5060 Ti, `crates/buzz-render/examples/vram_probe.rs`)

Vello's own transient buffers only (this example has no swapchain, no egui
textures, no other GPU context — see the root cause's "rest of the ~2 GB"
list for what else makes up the number the user reported):

| | before (old: `buffer_scale` set to `max_buffer_scale` immediately) | after (V2: starts at `1`) |
|---|---|---|
| at startup, before any frame | 0.7 MB allocated / 192 MB reserved | 0.7 MB allocated / 192 MB reserved |
| after the first (blank) frame, 1920x1080 | **1302 MB** allocated / 1728 MB reserved | **175 MB** allocated / 448 MB reserved |

"Before" reproduced by calling the new `GpuContext::start_generous()` right
after construction, matching exactly what pre-V2 `GpuContext::new` did (this
adapter's `max_buffer_scale` is `8`). Confirms the report's own
`transient_mb=1288` almost exactly.

**First grow hitch** (same example, growing `buffer_scale` `1 → 8` in one
call — the cumulative worst case across every level, not one step):
render at scale 1 1.30 ms, the growing frame 36.84 ms, steady-state already
at scale 8 0.78 ms → **hitch ≈ 36 ms**, once. A real session grows one
level (×2) at a time (`DrawCache::reconsider`), so a single step's hitch is
smaller than this; not separately measured.

**V4 step-down** (`crates/buzz-render/tests/buffer_scale_adaptive.rs`,
`shrink_to_gives_transient_memory_back_where_the_backend_can_report_it`):
1293 MB → 166 MB allocated after `GpuContext::shrink_to(1)` and a
`Device::poll` (the poll is required — wgpu only reclaims a dropped
resource's memory once it has confirmed, via a poll, that the GPU is done
with it; measuring immediately after `shrink_to` without polling first
showed memory going *up*, an artifact of measuring before reclamation
rather than the shrink not working).

**Heaviest regression FLA** (`barish1.fla`, via
`crates/buzz-app/tests/stage_matches_export_barish1.rs`): pixel parity
between the stage and the export still holds with this change (test
passes) — this test's harness now calls `ctx.start_generous()` explicitly
(see V2) so both sides render at the same, adapter-max scale exactly as
before; it does not independently confirm growth mid-session on a document
big enough to overflow scale `1`, which would need a document known to sit
between the scale-1 and scale-8 ceilings and was not available this pass.

**Not done**: DXGI `QueryVideoMemoryInfo` (wgpu's own
`generate_allocator_report` was sufficient and needed no new dependency);
the "MemoryHints::MemoryUsage" experiment in design point 6 (not attempted —
lower priority than V1–V5, no time left in this pass).

## Review log

- 2026-09-27 Opus: accepted. Checked: scale is thread-local in vello_encoding but every render sets it from its own GpuContext immediately before drawing (`set_scale_for_this_thread(self.buffer_scale)` at both render sites in buzz-render lib.rs), so it behaves per-context. Renderer rebuilt on shrink (vello not forked). Re-ran `stays_interactive` ×2 single-threaded: pass (the implementer could not — it saw a transient test-build break from a concurrent agent's mid-edit; `cargo test -p buzz-app --lib --no-run` compiles). Not done: DXGI whole-process measurement, MemoryHints::MemoryUsage experiment. Pre-existing, unrelated: buzz-export `small_artwork::a_200px_shape_is_drawn_as_before` (BARISH1 regression plan).
