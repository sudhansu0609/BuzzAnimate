# BuzzAnimate — Progress, Checkpoints & Implementation Plan

**Last updated:** 2026-08-12
**Current status:** Phase 0 complete and verified · Phase 1 not started

---

## 1. Core design principle

> **BuzzAnimate is Adobe Animate's workflow, rebuilt on a modern engine.**
> It is not a new kind of tool. An Animate user must be productive on day one.

Rules that follow from this, applied to every phase:

- **Use Animate's vocabulary.** Stage, pasteboard, layer, frame, keyframe, blank
  keyframe, tween span, symbol, instance, library, onion skin, scene. Never
  invent a new word for a thing Animate already names.
- **Match Animate's screen layout.** Toolbar left, stage centre, timeline
  bottom, panel dock right. Same default proportions.
- **Match Animate's default keyboard shortcuts** (V, A, Q, P, T, R, O, B, E, K,
  I, H, Z, F5, F6, F7, F8, Enter, …).
- **Match Animate's behaviour on ambiguity.** When unsure how something should
  work, do what Animate does.
- **Improvements go underneath, not on top.** Unbounded zoom, multicore and GPU
  change how fast and how far it goes — not where the buttons are.
- **Deviate only with a written reason**, recorded in §7.

---

## 2. Locked decisions

- **Stack:** Rust + wgpu + Vello
- **Geometry:** `f64` throughout via `kurbo`
- **UI:** egui + egui_dock for v1 (migration path noted in §7)
- **Animate files:** import modern `.fla` (zipped XFL) and `.xfl` folders
- **Also import:** `.pdf`, `.ai`, `.swf`
- **Flash:** full ActionScript authoring environment
- **v1 features:** drawing + timeline, symbols/library/tweens, rigging/IK, scripting
- **Export:** MP4/MOV (NVENC), PNG sequence, GIF/WebP, HTML5 Canvas/SVG
- **Legal:** clean-room. No decompiling Adobe binaries, no Adobe assets, icons
  or trademarks. Formats read are Adobe-published (SWF, ABC), ISO (PDF), or
  plain XML (XFL).

---

## 3. Verified environment

- CPU: Intel i7-14700K — 20 cores / 28 threads
- GPU: NVIDIA RTX 5060 Ti (Blackwell) — NVENC H.264 / HEVC / AV1
- RAM: 32 GB · OS: Windows 11 Pro 26200
- Rust 1.97.1 MSVC · MSVC Build Tools 2026 · Windows SDK 10.0.26100
- Node 25.9, Python 3.14, .NET 10, Git 2.36 also present

---

## 4. Checkpoint log — completed

> **Checkpoints are real commits, not just entries here.** Each completed phase
> is tagged, so any checkpoint can be returned to:
>
> ```sh
> git tag -n                 # list checkpoints
> git checkout phase-0       # return to one
> git diff phase-0 HEAD      # what changed since
> ```
>
> | Tag | Contents |
> |---|---|
> | `phase-0` | Engine foundation — unbounded zoom, multicore, GPU rasterisation |
> | `cp-1.1` | Document-space clipping — retires the Phase 0 culling limitation |
> | `cp-1.1b` | Boolean path operations with parallel tree reduction |
> | `cp-1.1-complete` | Path editing and parallel hit-testing; CP-1.1 done |
>
> Tags, not commit hashes, are the identifier: a hash written into this file
> can never name the commit that contains the file.

### ✅ CP-0.1 — Toolchain
- [x] Rust 1.97.1 `stable-x86_64-pc-windows-msvc` installed via rustup
- [x] `%USERPROFILE%\.cargo\bin` added to user PATH
- [x] MSVC linking verified by test compile
- [x] Confirmed `available_parallelism() = 28`
- **Note:** MSVC Build Tools 2026 already present — no multi-GB install needed

### ✅ CP-0.2 — Workspace
- [x] Cargo workspace, edition 2024, resolver 3
- [x] Crates: `buzz-geom`, `buzz-jobs`, `buzz-render`, `buzz-app`
- [x] Dependency versions pinned in `[workspace.dependencies]`
- [x] `profile.dev.package."*" = opt-level 3` (Vello is unusable unoptimised)
- **Gotcha found:** vello 0.9 needs wgpu ^29, egui 0.36 needs wgpu 30 →
  **egui pinned to 0.35**. Verified exactly one wgpu in `Cargo.lock`.

### ✅ CP-0.3 — Unbounded zoom camera (`buzz-geom`)
- [x] `Camera` with `f64` zoom/pan, **no clamp**
- [x] `RebasedTransform` — anchor + view split
- [x] `RenderSplit` — anchor + CPU scale + unit-scale GPU transform
- [x] `zoom_by_at` (cursor-anchored), `pan_screen`, `fit_to_rect`,
      `visible_doc_rect`, `screen_precision_px`
- [x] 10 tests, including a proof that rebasing beats fused composition
- **Precision model:** `precision_px ≈ |coord| × 2.22e-16 × zoom` — sub-pixel to
  ~1e12%, then linear decay. Surfaced live in the HUD.

### ✅ CP-0.4 — Job system (`buzz-jobs`)
- [x] Two rayon pools: Interactive (20) + Background (6), 2 threads reserved
- [x] Per-worker CPU time via Win32 `GetThreadTimes`
- [x] `Utilisation` with per-worker bars and `cores_busy`
- [x] Panic-safe job accounting
- [x] 7 tests including a real multicore proof
- **Correction made:** first version timed submitted jobs, which missed all
  nested `par_iter` work. Rewritten on OS thread CPU time.

### ✅ CP-0.5 — GPU adapter selection (`buzz-render`)
- [x] Enumerates DX12 + Vulkan, scores every adapter, logs the table
- [x] Disqualifies CPU rasterisers and virtual display drivers
- [x] Overrides: `--gpu <name|index>`, `--integrated`
- [x] 6 tests, decoupled from `AdapterInfo` so wgpu bumps don't break them
- **Live result:** 7 adapters found; selected `[2] RTX 5060 Ti DiscreteGpu/Dx12`;
  correctly rejected `Microsoft Basic Render Driver (Cpu)`

### ✅ CP-0.6 — Vello render loop + HUD (`buzz-app`)
- [x] winit window, wgpu surface, non-sRGB format selection
- [x] Vello → intermediate storage texture → blit → egui overlay
- [x] Wheel zoom at cursor, drag pan, `R` reset, `Esc` quit
- [x] HUD: GPU name, frame time, zoom, precision, per-core bars, stress button
- [x] Mandatory culling (oversized paths would need ~3e7 segments and hang)
- [x] Screen-relative flattening tolerance

### ✅ CP-0.7 — Phase 0 exit test
- [x] Headless GPU sweep with pixel readback, sharing the app's encode path
- [x] Determinism test, detail-survival test
- [x] **Result: ink coverage constant at ~4.01% from 2e2% to 2e14% zoom**
- [x] 44 tests pass workspace-wide · clippy clean
- **Bug found and fixed:** GPU time was 25 ms/frame at 1e12% with detail loss.
  Cause was the magnification being applied by the GPU in `f32`. Moving it to
  CPU `f64` → **0.9 ms** and full detail restored (28× faster).

---

### ✅ CP-1.1a — Document-space clipping
- [x] `buzz-geom::clip::RenderClip` — adaptive Bézier clipping to a rectangle
- [x] Uses the convex-hull property: control-point bbox is a cheap conservative
      reject, so in-view curves are emitted untouched and cost nothing
- [x] Invisible segments collapse to one line **to their endpoint**, which
      preserves winding so enclosing fills still fill
- [x] Bounds **both** segment count (budget 20 000) and coordinate magnitude
      (clamped into an expanded rect)
- [x] Wired into `SceneBuilder` before the anchor subtraction and scale
- [x] `culled_huge` removed from the render path — culling is now purely an
      optimisation, never load-bearing for correctness
- [x] 11 clipping tests, including winding-number preservation over a sampled
      grid across five shape cases (no rasteriser involved)
- **Measured:** items drawn at 2e14% went **70 → 213** (shapes that used to be
  dropped are now correctly clipped and drawn), GPU time **0.8–1.7 ms**, ink
  coverage unchanged at 4.01%
- **Regression tests added:** an oversized background fills the frame at 100%
  ink at 1e2/1e6/1e12% (Phase 0 made it vanish); a huge circle's edge through
  the view yields exactly 50.0% coverage
- **Checked, not assumed:** kurbo grows a circle's segment count as the *sixth
  root* of `radius/tolerance`, so `to_path` at a 5e-12 tolerance is ~200 cubics.
  Clipping after flattening is therefore the correct order.

### ✅ CP-1.1b — Boolean operations
- [x] `buzz-geom::boolean` — Union, Intersect, Difference, Xor on filled paths
- [x] NonZero and EvenOdd fill rules
- [x] `boolean_many` uses a **rayon tree reduction**: `n` paths combine in
      `log n` dependent rounds, not `n` sequential steps
- [x] Difference is correctly excluded from tree reduction — it is not
      associative, so it stays a left fold
- [x] Curves are **refitted** after the operation via `kurbo::simplify`, so
      results stay editable rather than polygonal
- [x] `BooleanOptions::for_shape_size` derives tolerance from geometry, so a
      2-unit glyph and a 10 000-unit background both behave
- [x] 18 tests: area identities for every operator, holes, disjoint shapes,
      non-commutativity, empty and degenerate inputs, parallel-vs-sequential
- **Design decision:** delegated to `i_overlay` rather than writing Bézier-native
  booleans. Robust booleans fail on tangencies, self-intersection and
  near-degenerate curves, and a subtly wrong answer corrupts artwork silently.
  Cost is a flatten/refit round trip; tolerance is the quality control.
- **Bug found and fixed:** kurbo's default corner threshold is ~1 milliradian,
  so every vertex of a freshly flattened path counted as a corner and refitting
  returned pure lines. Raised to a tangent of 0.2 (~11°), derived from the
  `2·√(2t/r)` turn angle that flattening leaves. A test guards that real 90°
  corners still survive.
- **Precision note:** `i_overlay` snaps to an integer grid, which is fine
  because booleans are a document edit at authoring scale — never a render-path
  operation at 1e12× zoom.

### ✅ CP-1.1c — Path editing operations
- [x] `buzz-geom::edit` mapped directly to Animate menu commands:
      `outline_stroke` (Convert Lines to Fills) · `expand_fill` (Expand Fill) ·
      `smooth` (Smooth) · `straighten` (Straighten)
- [x] `path_length` and `point_at_fraction`, needed for Phase 4 motion paths
- [x] **`expand_fill` is built from stroking + booleans**, not curve offsetting.
      Direct offsetting means handling joins, caps, cusps and the
      self-intersections that appear when the offset exceeds a local radius of
      curvature. Stroking the boundary at `2·|amount|` and unioning (or
      subtracting) reuses two already-tested components instead.
- [x] **Bug found and fixed — corrupted geometry.** Refitting a *correct*
      polygon spanning `-5..105` produced a path spanning `-5..1071`, a tenfold
      overshoot on a rounded corner. kurbo documents that its fitter "works best
      if the source path is very smooth". `refit_checked` now verifies every fit
      against the source bounds and area and falls back to the polygon when it
      diverges. A correct polygon always beats a corrupted curve.

### ✅ CP-1.1d — Parallel hit-testing
- [x] `buzz-geom::hit` — fill (winding), stroke (distance with tolerance),
      nearest-point queries for subselection and snapping
- [x] `hit_test_topmost` / `hit_test_all` / `select_in_rect`, parallel above 64
      targets and sequential below, with a test asserting both agree
- [x] Stroke takes priority over fill, matching Animate's Selection tool
- [x] Locked and hidden layers are skipped via `selectable`
- [x] Marquee distinguishes enclosing from crossing selection
- [x] Tolerance is in **document units**, supplied by the caller from the
      current zoom — without it a hairline would be unclickable
- [x] **Bug found and fixed — dead code.** kurbo's `Rect::intersect` *clamps*
      to zero size (`x1.max(x0)`) rather than returning negative extents, so the
      `width() < 0.0` off-screen rejection copied into `demo.rs` could never
      fire. Replaced with `Rect::overlaps`. Effect at 2e14% zoom: items drawn
      **213 → 61** for byte-identical output — roughly 3.5× less work.

---

## 5. Current metrics

| Measure | Value |
|---|---|
| Zoom verified | 2×10¹⁴% (Animate: 2 000%) |
| Render quality across 13 decades | constant ~4.01% ink |
| GPU frame time at 1e12% | 0.9 ms |
| CPU encode time | ~0.10 ms, flat across all zooms |
| Threads in use | 20 interactive + 6 background |
| Items drawn at 2e14% | 61 of 224, identical output (70 before clipping, 213 before the overlap fix) |
| Tests | 104 passing, clippy clean |
| Rust source | ~5 400 lines |

---

## 6. Implementation plan

### ⬜ Phase 1 — Geometry & document core
*No UI yet. This is the foundation everything else sits on.*

- [x] **CP-1.1** `buzz-geom` expansion — **complete**
  - [x] **Document-space clipping** (retires the culling limitation, §7)
  - [x] Boolean ops (union, subtract, intersect, xor), parallel tree reduction
  - [x] Path offsetting, simplification, smoothing
  - [x] Parallel hit-testing; stroke hit-testing with tolerance
- [ ] **CP-1.2** `buzz-scene` — the document model
  - [ ] Copy-on-write scene graph (`Arc` structural sharing)
  - [ ] **Layer model matching Animate** — see §8.2
  - [ ] Groups, transforms, z-order, depth
  - [ ] R-tree spatial index (`rstar`), rebuilt off-thread
- [ ] **CP-1.3** `buzz-doc` — persistence
  - [ ] `.buzz` format (zip + JSON/binary), versioned
  - [ ] Snapshot-based undo/redo (snapshots *are* the history)
  - [ ] Background autosave and crash recovery
- [ ] **Exit test:** build a 10 000-shape document, boolean-op it across all
      cores, save, reload, undo to empty, redo — byte-identical round trip

### ⬜ Phase 2 — Stage, tools & UI shell *(Animate parity begins)*
- [ ] **CP-2.1** Application frame — see §8.1 for the exact layout
  - [ ] Menu bar with Animate's menu structure
  - [ ] Dockable panels via `egui_dock`, saved workspace layouts
- [ ] **CP-2.2** **Stage** — see §8.3
  - [ ] Stage rectangle + pasteboard (work area)
  - [ ] Document properties: dimensions, background colour, frame rate
  - [ ] Rulers, guides, grid, snapping
  - [ ] Zoom control with Animate's presets **plus unbounded entry**
  - [ ] Scenes
- [ ] **CP-2.3** Toolbar with Animate's tools and shortcuts — see §8.4
- [ ] **CP-2.4** Drawing and editing
  - [ ] Merge-shape vs object-drawing modes (an Animate-specific behaviour)
  - [ ] Strokes, fills, gradients, swatches
  - [ ] Free transform, subselection, path editing
- [ ] **Exit test:** reproduce a reference Animate drawing tool-for-tool

### ⬜ Phase 3 — Timeline & frame animation
- [ ] **CP-3.1** Timeline panel — see §8.5
  - [ ] Layer list + frame grid, playhead, frame numbers, fps, elapsed time
  - [ ] Keyframe / blank keyframe / frame span rendering in Animate's style
  - [ ] F5 / F6 / F7 / Shift-F5 / Shift-F6 behaviour
- [ ] **CP-3.2** Layer types — see §8.2
  - [ ] Normal, folder, mask, masked, guide, guided
  - [ ] Show/hide, lock, outline view, layer colour, layer depth
- [ ] **CP-3.3** Playback
  - [ ] Playback decoupled from render rate; loop; frame stepping
  - [ ] Scrubbing with speculative prefetch across cores
  - [ ] Onion skinning + outlines + edit multiple frames (parallel ghosts)
- [ ] **Exit test:** 500-frame, 20-layer document scrubs at 60 fps

### ⬜ Phase 4 — Symbols, library & tweens
- [ ] **CP-4.1** Symbols: Graphic, MovieClip, Button; nested timelines;
      instance overrides; symbol editing mode; breadcrumb bar
- [ ] **CP-4.2** Library panel: folders, search, previews, usage counts,
      background-generated thumbnails
- [ ] **CP-4.3** Tweens: classic, motion (with motion paths), shape (with
      shape hints); editable easing curves; Motion Editor
- [ ] **Exit test:** a character walk cycle built entirely from symbols + tweens
- **Note:** must land before Phase 5 — importers need somewhere to put data

### ⬜ Phase 5 — Importers
- [ ] **CP-5.1** `.fla` / `.xfl` — unzip, parse `DOMDocument.xml` +
      `LIBRARY/*.xml`, map onto the Phase 4 model, extract `bin/` media,
      parallel per-symbol parse, emit a fidelity report
- [ ] **CP-5.2** `.pdf` / `.ai` — content-stream path extraction via
      `pdfium-render`; `.ai` v9+ is PDF internally so one parser covers both;
      pre-v9 PostScript fails with a clear message
- [ ] **CP-5.3** `.swf` — `DefineShape` / `DefineSprite` / `PlaceObject` /
      bitmaps / fonts → editable vectors and library symbols
- [ ] **Exit test:** import a real `.fla` and compare frame-by-frame against an
      Animate reference render

### ⬜ Phase 6 — Export
- [ ] **CP-6.1** PNG sequence — parallel across all 28 threads, any resolution
- [ ] **CP-6.2** MP4 / MOV — NVENC (`h264_nvenc` / `hevc_nvenc` / `av1_nvenc`)
      via `ffmpeg-sidecar`, N frames in flight
- [ ] **CP-6.3** GIF / WebP — palette quantisation, animated WebP
- [ ] **CP-6.4** HTML5 Canvas / SVG — scene graph → JS + small runtime player
- [ ] **Exit test:** one document exported to all four, all four play correctly

### ⬜ Phase 7 — Rigging
- [ ] **CP-7.1** Bone tool, armatures over shapes and symbol chains
- [ ] **CP-7.2** FABRIK IK solver with angle limits and pin constraints
- [ ] **CP-7.3** Puppet warp via Moving Least Squares mesh deformation
- [ ] **CP-7.4** Vertex weight binding, armature tweening
- [ ] **Exit test:** rig and animate a character arm; IK solves for 50
      armatures in parallel within one frame budget

### ⬜ Phase 8 — Scripting & ActionScript
- [ ] **CP-8.1** Plugin API (JSFL equivalent) — `rquickjs` sandbox exposing
      document / timeline / library / selection; runs off the UI thread and
      submits through the same command queue so undo works uniformly
- [ ] **CP-8.2** Actions panel — `tree-sitter` highlighting, autocomplete,
      error squiggles, frame scripts, symbol class linkage
- [ ] **CP-8.3** AS3 runtime — embed `ruffle_core`'s AVM2 (MIT/Apache)
- [ ] **CP-8.4** AS3 compiler — **bundle Apache Royale/Flex `mxmlc`** first
      (weeks, costs a bundled JRE) rather than writing AS3→ABC from scratch
      (6–12 months). Native Rust ABC emitter later to drop the Java dependency.
- [ ] **Exit test:** frame script drives a MovieClip; project publishes and runs

---

## 7. Known issues & deviations

| # | Item | Status |
|---|---|---|
| 1 | ~~**Oversized paths culled, not clipped.**~~ | ✅ **Resolved in CP-1.1** by `RenderClip` |
| 2 | **egui pinned to 0.35.** 0.36 requires wgpu 30; vello 0.9 requires wgpu 29. Two wgpu majors cannot share a device. | Blocked on vello |
| 3 | **egui is immediate-mode**, not ideal long-term for a pro creative tool. Chosen to reach a working app fast. | Revisit after Phase 4 |
| 4 | **`f64` precision floor** — sub-pixel to ~1e12%, linear decay after. | Documented, by design |
| 5 | **Legacy binary `.fla`** (CS4 and older, OLE2) unsupported. | Out of scope |
| 6 | **`.fla` write-back** not planned for v1 — import only. | Deferred |
| 7 | `to_render_space` allocates two paths per shape per frame. | Optimise if profiling says so |

---

## 8. Adobe Animate parity specification

*The reference for every UI decision. Build to this, not to intuition.*

### 8.1 Window layout
- **Menu bar:** File · Edit · View · Insert · Modify · Text · Commands ·
  Control · Debug · Window · Help
- **Left:** vertical Toolbar
- **Centre:** Stage + pasteboard, with an edit/scene breadcrumb bar above
- **Bottom:** Timeline (layers left, frame grid right)
- **Right:** dockable panel column — Properties, Library, Color, Swatches,
  Align, Transform, Info, Actions
- Workspace presets, saveable and resettable

### 8.2 Layers — must match Animate exactly
- **Types:** Normal · Folder · Mask · Masked · Guide · Guided
- **Per-layer controls:** Show/Hide (eye) · Lock (padlock) · Show as outlines
  (square, tinted by layer colour)
- **Properties:** name, layer colour, outline mode, layer height, layer depth
- **Operations:** add, delete, duplicate, reorder by drag, nest into folders,
  convert to mask/guide, distribute to layers
- Multi-select, and lock-others / hide-others

### 8.3 Stage
- White (configurable) stage rectangle on a grey **pasteboard**
- Objects on the pasteboard are editable but do not render in output
- Document properties: dimensions, background colour, frame rate, ruler units
- Rulers, draggable guides, grid, snap to guides/grid/objects/pixels
- Zoom presets: Fit in Window · Show Frame · Show All · 25 · 50 · 100 · 200 ·
  400 · 800% — **plus a free field with no upper bound (our addition)**
- Multiple Scenes

### 8.4 Toolbar
- Selection `V` · Subselection `A` · Free Transform `Q` · Gradient Transform
- Lasso `L` · Pen `P` · Text `T` · Line `N`
- Rectangle `R` · Oval `O` · PolyStar · Rectangle Primitive · Oval Primitive
- Pencil `Y` · Brush `B` · Paint Brush · Width tool
- Bone `M` · Asset Warp · Paint Bucket `K` · Ink Bottle `S`
- Eyedropper `I` · Eraser `E` · Camera `C` · Hand `H` · Zoom `Z`
- Stroke and Fill colour wells, swap/default/none
- Tool options area at the bottom, contextual to the active tool

### 8.5 Timeline
- Layer list on the left, frame grid on the right, shared vertical scroll
- Playhead with frame ruler; current frame, fps, elapsed time readouts
- **Frame rendering conventions:** keyframe = filled circle · blank keyframe =
  hollow circle · end of span = hollow rectangle · motion tween = blue span ·
  classic tween = purple with arrow · shape tween = green with arrow
- Controls: onion skin, onion skin outlines, edit multiple frames, modify
  markers, loop range
- Shortcuts: `F5` insert frame · `F6` insert keyframe · `F7` insert blank
  keyframe · `Shift+F5` remove frame · `Shift+F6` clear keyframe
- Right-click frame menu matching Animate's

### 8.6 Panels
- **Properties** — fully contextual: document / tool / selection / frame /
  symbol instance
- **Library** — folders, search, preview, usage count, linkage
- **Color** — RGB/HSB, alpha, gradients, fill types
- **Align · Transform · Info · Swatches · Actions**

---

## 9. Commands

```sh
cargo run --release -p buzz-app              # run
cargo test --workspace                       # 44 tests
cargo clippy --workspace --all-targets       # lint
cargo test -p buzz-app --test headless_zoom --release -- --nocapture
```

---

## 10. Next action

**Begin CP-1.2** — `buzz-scene`, the document model:

- Copy-on-write scene graph with `Arc` structural sharing. This is the decision
  the whole multithreading model rests on: the renderer reads a snapshot with
  zero locks while the document thread builds the next one, and undo becomes
  nearly free because old snapshots *are* the history.
- Animate's six layer types — Normal, Folder, Mask, Masked, Guide, Guided — with
  show/hide, lock, outline view, layer colour and depth (§8.2).
- Groups, transforms, z-order.
- R-tree spatial index (`rstar`), rebuilt off-thread, feeding the hit-testing
  built in CP-1.1d.
