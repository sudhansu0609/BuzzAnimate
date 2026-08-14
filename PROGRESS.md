# BuzzAnimate — Progress, Checkpoints & Implementation Plan

**Last updated:** 2026-08-14
**Current status:** Phases 0–5 complete (gaps in §7), plus fluid, pattern and
art brushes. All three importers — `.fla`/`.xfl`, `.swf`, `.pdf`/`.ai` — read,
merge into an open document, and report what they could not bring across.
**Phase 7 is done:** armatures, FABRIK inverse kinematics with joint limits and
pins, skinning, puppet warp, and poses that tween. **Phase 8 started out of
order:** scripting (CP-8.1) runs sandboxed JavaScript through Animate's own
`fl` / `document` API. **Images now come out** — a frame or a whole range
exports as PNG, which is the first half of Phase 6. **Masks clip**, and
**sound works**: a soundtrack on the main timeline is audible from inside any
symbol at any depth, draws its waveform in the timeline, and drives automatic
lip sync.

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
> | `cp-1.2` | Document model: COW scene, Animate layers, R-tree index |
> | `phase-1` | `.buzz` format, undo/redo, autosave; **Phase 1 complete** |
> | `phase-2` | Application shell, stage, toolbar, drawing and editing |
> | `phase-3` | Timeline, keyframes, playback, onion skinning, camera |
> | *(untagged)* | Symbols, instances and tweens persisted — format version 3 |
> | `phase-5` | Library, symbol editing, tweens **and** the three importers |
>
> Tags, not commit hashes, are the identifier: a hash written into this file
> can never name the commit that contains the file.
>
> **`phase-4` has no tag of its own.** Phases 4 and 5 were finished in one
> working tree, and the files overlap — the Library panel, `Scene::merge` and
> the importers all touch `buzz-scene/src/lib.rs`. Splitting them after the
> fact would have produced an intermediate commit that did not build, which is
> worse than an honest note: a checkpoint you cannot check out is not a
> checkpoint. `phase-5` is the checkpoint for both.

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

### ✅ CP-1.2 — Document model (`buzz-scene`)
- [x] **Copy-on-write `Scene`** — snapshotting is `O(1)`: cloning bumps the one
      `Arc` around the layer list, and copy-on-write descends into a layer, then
      an object, only when an edit forces it
- [x] Revision counter on every edit, so derived data can detect staleness
- [x] `Object` / `ObjectKind` — shapes and nestable groups, with transform
      accumulation and correct rotated bounds
- [x] Stroke width included in bounds; hairlines correctly excluded
- [x] **Animate's six layer types** — Normal, Folder, Mask, Masked, Guide,
      Guided — with show/hide, lock, outline view, layer colour, layer height,
      folder nesting and reordering
- [x] **Positional mask resolution**: a mask claims the unbroken run of masked
      layers below it, stopping at the first that is not. This is Animate's own
      rule and what `.fla` files depend on — not an invented explicit link.
- [x] Guide/guided resolved by the same positional rule
- [x] Visibility and locking resolve through folder ancestors, with a bounded
      walk so a corrupt parent cycle cannot hang
- [x] `StageProperties` matching Animate's defaults (550×400, 24 fps)
- [x] `IdAllocator` with `reserve_above`, so importer-assigned ids are never
      reused by later edits
- [x] **R-tree spatial index** (`rstar`), bulk-loaded, with a test asserting it
      agrees with a brute-force scan over 5 000 objects
- [x] Index records the revision it was built from and reports itself stale
- [x] 53 tests
- **Proven, not asserted:** a test builds an index for a 5 000-object snapshot
  on another thread *while the main thread keeps editing*, then confirms the
  index describes the snapshot and reports itself stale against the new
  revision. That is the payoff of the immutable model.
- **Corrected assumption:** the first sharing test expected snapshotting to add
  one refcount per object. It adds none — the structure is shared wholesale
  behind a single `Arc`, so snapshots are cheaper than the design assumed.

### ✅ CP-1.3 — Persistence, undo and autosave (`buzz-doc`) — **Phase 1 complete**
- [x] **`.buzz` container** — zip with `mimetype` stored uncompressed first
      (ODF/EPUB convention, so the file is identifiable without unzipping),
      `meta.json`, `document.json`, and `library/` + `media/` reserved for
      Phases 4–5
- [x] **Saves are atomic** — temp file plus rename, so an interrupted save
      cannot leave a truncated file where the user's work was
- [x] **Separate DTO layer**, deliberately. Deriving `Serialize` on the runtime
      model would weld the format to internal struct layout, so renaming a
      field would silently break every saved document.
- [x] **Paths stored as SVG strings** — compact, diff-friendly, matches
      SVG/XFL, and *lossless*: kurbo formats with `Display for f64`, which
      emits the shortest exactly-round-tripping string. Tested with extreme
      coordinates (1e-12 through 1e9).
- [x] **Colours as `#RRGGBBAA`** rather than peniko's own encoding, so the
      format does not move when peniko does
- [x] Format version checked on load; a future version is refused, not misread
- [x] **Undo/redo** with labelled steps for the History panel
- [x] **Drag coalescing** — 200 mouse-move edits collapse to one undo step;
      `end_gesture` breaks the run so a second drag stays separate
- [x] Bounded stack at Animate's default depth of 100
- [x] **Dirty state is revision-based, not a flag** — so undoing back to the
      saved state correctly reports *clean* again
- [x] **Autosave** to a separate recovery file, never over the user's document
- [x] `AutosavePlan` is `Send`: the caller snapshots, hands it to the
      background pool, and keeps editing. Tested by writing a 500-object
      snapshot on another thread while the main thread adds 200 more shapes.
- [x] Recovery discovery on startup, linked back to its source document
- [x] `Document::edit` is the single mutation entry point, so no code path can
      change the document without recording undo
- [x] 56 tests, including a full edit → save → crash → recover cycle

### ✅ Phase 2 — Application shell, stage, tools and drawing
*Substantially complete. Remaining gaps are listed explicitly in §7.*

**CP-2.1 — window frame**
- [x] `buzz-ui` crate: theme, command/shortcut map, tool catalogue, snapping,
      draw style, selection — all testable without a window
- [x] Menu bar with Animate's structure (File · Edit · View · Insert · Modify)
- [x] Layout: toolbar left, stage centre, timeline bottom, panels right,
      status bar with the unbounded zoom field
- **Deviation:** uses egui's own panels, not `egui_dock`. Animate's arrangement
  is fixed, and a generic tab-docking UI reads as *less* like Animate, not
  more. `egui_dock` was dropped from the dependencies.

**CP-2.2 — stage**
- [x] Stage rectangle on a grey pasteboard, document properties (size, fps,
      background)
- [x] Rulers with adaptive tick spacing; drag off a ruler to create a guide
- [x] Guides (lockable), grid with zoom-adaptive spacing
- [x] Snapping to guides / grid / objects / pixels
- [x] Zoom presets **plus** an unbounded field, and a live precision readout
- [x] Grid and ruler drawing is bounded, so extreme zoom cannot try to draw
      millions of lines

**CP-2.3 — toolbar**
- [x] All 21 Animate tools with their letter shortcuts (V A Q L P T N R O Y B
      M K S I E C H Z), verified against Animate by test
- [x] Unavailable tools are greyed with a tooltip saying which phase brings
      them — a tool that looks available but does nothing is worse
- [x] Stroke/fill colour wells with swap (X) and default (D)

**CP-2.4 — drawing and editing**
- [x] Rectangle, Oval, PolyStar, Line, Pencil, Brush, Eraser
- [x] Shift constrains squares, circles and 45° lines
- [x] Selection, marquee, move, Free Transform with anchored scaling
- [x] **Subselection with anchor editing** — dragging an anchor carries both
      adjacent Bézier handles, so the curve slides instead of kinking
- [x] Paint Bucket, Ink Bottle, Eyedropper, Hand, Zoom
- [x] **Merge Shape vs Object Drawing** — same-coloured fills fuse, a different
      colour cuts. This is what CP-1.1b's booleans were built for.
- [x] Group/ungroup, arrange (front/forward/backward/back), delete, duplicate
- [x] Convert Lines to Fills, Expand Fill, Smooth, Straighten
- [x] Layers, Properties and Color panels, swatches with recent colours
- [x] Open/Save/Save As dialogs; autosave running on the background pool

**Verified by screenshot, not just by test:** the window was captured and
inspected. That caught two defects tests could not — most tool glyphs rendered
as empty boxes because egui's bundled fonts lack those symbols, and the status
bar sat under the taskbar. Buttons now show their shortcut letter, which always
renders and teaches the keyboard at the same time.

### ✅ Phase 3 — Timeline, playback and the camera

**CP-3.1 — frame model**
- [x] `LayerTimeline` replaces the flat object list: keyframes owning spans
- [x] Three genuinely distinct frame states — beyond the span, inside a span,
      and a keyframe. A **blank keyframe is not the same as no frame**: it
      actively clears what the previous keyframe showed.
- [x] Animate's operations with Animate's semantics: **F6 duplicates** the
      previous artwork (that is how you make a copy to modify), **F7 starts
      empty**, F5/Shift+F5 insert and remove frames shifting later keyframes,
      Shift+F6 merges a keyframe into the span before it
- [x] Frame 0's keyframe cannot be removed — early frames would have nothing
      to show
- [x] Drawing inside a span edits the keyframe that *owns* it, so painting on
      frame 7 of a span beginning at frame 5 modifies frame 5
- [x] Binary search for frame lookup, tested on a 10 000-frame timeline

**CP-3.2 — timeline panel**
- [x] Layer rows plus frame grid, playhead, frame numbers, fps, elapsed time
- [x] Animate's drawing conventions: filled circle = keyframe, hollow circle =
      blank keyframe, shaded cell = span, hollow rectangle = span end
- [x] Scrub by dragging the ruler; click a cell to move the playhead and select
      the layer
- [x] Transport controls, and F5/F6/F7 buttons alongside them
- [x] Column count bounded, so a 50 000-frame document still draws quickly

**CP-3.3 — playback and onion skinning**
- [x] Playback advances on **elapsed time, not frames rendered**, so a document
      plays at its authored rate on a 60 Hz or a 144 Hz display
- [x] Looping on by default; without it playback stops on the last frame
- [x] Onion skinning with configurable before/after counts, fading with
      distance, and an outline mode; suppressed during playback
- [x] Scrubbing stops playback, as in Animate

**CP-3.4 — the camera**
- [x] A document-level keyed view transform, so moving it is part of the
      animation rather than part of your view of the stage
- [x] Camera tool enabled and wired: dragging moves the camera, inverted like a
      real camera
- [x] Interpolation that behaves: position linear, **zoom geometric** so 1× to
      4× passes through 2× rather than 2.5×, rotation by the **shortest way
      round** so 350° to 10° turns forward 20°
- [x] Values hold outside the keyed range instead of snapping to the origin
- [x] Camera edits are undoable and are saved with the document

**Bug found and fixed — silent loss of undo and index invalidation.**
`Scene::stage` and `Scene::camera` were public fields, so writing to them never
bumped the revision. Camera moves were therefore not undoable, and resizing the
stage did not invalidate the spatial index. Both are now private behind
`stage_mut()` / `camera_mut()`, which bump — and the compiler immediately found
two more places that had been writing silently.

**Format version 2.** Layers hold keyframes and documents hold a camera track.
Version 1 files still load, their flat object list becoming a single keyframe
at frame 0, which is exactly what it meant.

### ✅ Phase 4 — Symbols, library and tweens

**CP-4.1 — symbols and instances**
- [x] Graphic, MovieClip and Button symbols, each with its **own layer stack**,
      so a symbol is a document inside the document
- [x] Instances are references, not copies: `SymbolInstance` carries only what
      is per-placement — first frame, loop mode, colour effect
- [x] **Symbol editing mode.** `Scene::layers()` answers "the timeline the user
      is looking at", so the stage, the timeline, selection and every tool
      follow you into a symbol without any of them knowing they did. Threading
      a context parameter through seventy call sites would be the same
      behaviour with seventy more places to forget it.
- [x] `Scene::stage_layers()` reaches the document's own timeline regardless of
      context — and **that is what saving uses**. Without the distinction, a
      save made inside a symbol would replace the main timeline with it.
- [x] Which symbol is open is **not document state**: never serialised, never
      bumps the revision, excluded from `PartialEq`. `Document::edit_view` is
      the entry point, and a debug assertion catches anything that tries to
      change artwork through it. Opening a symbol must not mark a document
      dirty or land in the undo history.
- [x] Breadcrumb above the stage, and Animate's F8 / Ctrl+F8 / Ctrl+E
- [x] **F8 moves the artwork into the symbol** rather than copying it, and
      rebases it so the registration point is the selection's top-left — that
      is what makes it a conversion rather than a duplication
- [x] Re-entering a symbol already on the edit path jumps back to that level
      instead of pushing again, so a cyclic file cannot be walked forever
- [x] Nested instances render, with colour effects **composed** down the chain,
      bounded at 12 levels so a self-referencing symbol cannot exhaust the
      stack

**CP-4.2 — library panel**
- [x] Folder tree, search across the whole library, per-symbol use counts,
      rename, duplicate, delete, move between folders, place on stage
- [x] **The tree is derived, not stored.** A symbol keeps a folder path string,
      as XFL does; the tree is rebuilt each frame from those strings, so it
      cannot drift out of step with the symbols it describes.
- [x] Deleting a symbol that is still placed says how many instances it leaves
      drawing nothing, rather than refusing or silently orphaning them
- [x] A test walks the tree and asserts it reaches every symbol exactly once —
      a symbol invisible in the panel but present in the file is the failure
      mode worth guarding

**CP-4.3 — tweens**
- [x] Classic, motion and shape tweens; `LayerTimeline::resolved_at` applies
      them **in the render path**, so tweened frames draw without existing
      anywhere in the document
- [x] Untweened frames return **borrowed** artwork, so the common case
      allocates nothing; only a tween builds new objects
- [x] Easing: Animate's -100..100 strength slider, plus cubic Bézier for an
      imported custom curve
- [x] Rotation interpolates the shortest way round; shape tweens interpolate
      geometry, degrading predictably when structures differ
- [x] Timeline draws Animate's colours — motion blue, classic purple, shape
      green — with an arrow across the span
- [x] **A tween with no following keyframe is drawn dashed**, not solid.
      Interpolating towards nothing is the usual reason a new tween appears to
      do nothing, so the model reports it (`TweenSpan::is_complete`) rather
      than hiding it.

**Instance properties.** Animate's four named colour effects are **recovered**
from the stored transform rather than stored alongside it. The document keeps
only the multiply/add pair — that is what tweening interpolates and what
nesting composes — and a stored *name* would have to be kept in step with a
transform that tweening changes every frame. `ColorEffect::from_transform`
inverts the mapping, next to the constructors it inverts, with a round-trip
test. Positive brightness and a white tint are provably the same six numbers,
so that pair is documented as indistinguishable rather than papered over.


**Verified on screen, not just by test.** A fixture document
(`buzz-doc/tests/make_fixture.rs`, `--ignored`) was built and opened, and the
window captured. That caught two font defects tests could not see: `📁` and
`▸` have no glyph in egui's bundled fonts and rendered as empty boxes — the
same class of defect Phase 2 found. Both are gone; `🔍`, `➕`, `🗑`, `▼` and
`▶` were confirmed to render. The capture also confirmed the four tween span
styles, the library tree and a red 40% tint read back correctly out of a
*saved and reloaded* document.


### ✅ Layer depth — the 3D arrangement of layers

Listed as done under CP-3.2 and §8.2 since Phase 3, and **it was not**: `Layer`
had no depth field at all. Now it does.

**The model**
- [x] `Layer::depth`, in document units. Zero is the focal plane; positive is
      further from the camera, negative is nearer.
- [x] `CameraTrack::focal_distance` — how far the camera sits from that plane,
      and therefore how violent the perspective is. A shorter distance
      exaggerates it exactly as a wider lens does.
- [x] Straight pinhole projection: a layer at distance `f + depth` renders at
      `f / (f + depth)`. Depth 0 returns **exactly** 1.0 with no arithmetic to
      round it away, so a document that never touches depth is bit-identical to
      one from before the feature existed.
- [x] **Parallax is not a second rule bolted on.** The depth transform is the
      camera transform with the zoom scaled by that factor, so a distant layer
      is both drawn smaller *and* slides less when the camera pans — the two
      effects fall out of one projection, and cannot disagree.
- [x] Depth works **without a camera keyframe**. With no keys the camera still
      has a position — the middle of the stage — so pushing a layer back does
      something immediately rather than nothing until you key the camera.
- [x] A layer at or behind the camera is **not drawn**, rather than magnified
      by a perspective divide approaching zero into a wall of colour.

**Picking**
- [x] Depth draws a layer's artwork away from where its geometry sits, so the
      click is moved the same way in reverse before it is tested. Without this
      a layer pushed back is visible but unclickable, and one pulled forward is
      selected by clicking empty space beside it. The pick tolerance is scaled
      with the layer, so a distant layer is not easier to hit than a near one.

**The Layer Depth panel**
- [x] A **side-on view** of the arrangement: the camera at the left, depth
      running right, each layer a plane whose height falls off exactly as the
      renderer's perspective does, with sight lines to the selected one.
- **Why side-on rather than through the camera.** The stage already shows what
  the camera sees, and it cannot answer the question you open this panel to
  ask: a layer twice as far and twice as big looks identical through the lens.
  From the side, two layers at the same depth land on the same line.
- [x] Per-layer depth, a camera-depth slider, and Flatten / Distribute
- [x] Clicking a plane selects that layer

**Deliberately not done:** depth does **not** reorder drawing. Paint order is
still the timeline's layer order, exactly as in Animate — pushing a foreground
layer back shrinks it without sending it behind anything.

**Bug found and fixed — every layer the same colour.** The Layer Depth view
made it obvious: all five planes came out orange. Layer colours were indexed by
*id*, and ids are shared with objects, so a document with seven shapes per
layer strode the ids by eight and handed every layer palette entry 1. Colours
are now assigned by position. The defect was equally present in the timeline's
colour chips and in outline view; it took a picture of the layers side by side
to notice.

**Proved on the GPU:** `headless_build_up.rs` renders through the same path the
window uses and reads the pixels back — a layer at depth 1000 draws at half
width, one at the camera plane draws nothing at all, and panning the camera
sweeps a near layer furthest and a distant one least. Then confirmed on screen
with a five-layer fixture: sky, hills, trees, stage and foreground, each at its
own scale, sliding at its own rate as the camera pans.

### ✅ Brushes — fluid, pattern and art

A Phase 2 follow-up, taken out of order because the brush is the tool an
animator spends the most time holding.

**The fluid brush** (`buzz-geom::brush`)
- [x] Width varies along the stroke: with pen pressure where the device
      reports it, and with **speed** where it does not
- [x] Speed is the default, and the reason matters: a mouse reports a constant
      pressure of 1.0, so a pressure-driven brush on a mouse paints a dead
      constant width. Speed is what makes a *mouse* stroke look drawn.
- [x] Tapered ends, on a square-root curve rather than linear — a linear taper
      reads as a wedge; a brush end is closer to an ellipse
- [x] Smoothing that is **symmetric**, so it does not lag the pointer, and
      never moves the endpoints
- [x] A tap paints a dot, as Animate does
- **Design decision — Catmull-Rom, not a curve fit.** Curves are built by the
  closed-form Catmull-Rom-to-Bezier construction, which passes exactly through
  every input point and *cannot* overshoot. CP-1.1c found kurbo's fitter
  turning a correct path spanning `-5..105` into one spanning `-5..1071` when
  handed input that was not smooth — and freehand input never is. Fewer
  segments would not be worth a brush stroke that occasionally explodes.
- **Self-intersections are left alone.** A stroke that doubles back merges with
  itself under the non-zero fill rule, which is what paint does. Resolving
  them would cost a boolean per stroke for no visible difference.

**Pattern and art brushes**
- [x] Six built-in shapes — dot, dash, leaf, star, arrow, diamond — plus
      **Create Brush From Selection**, Animate's own way of making one
- [x] Stamps rotate to follow the tangent, and are centred on the stroke
- [x] Art brushes stretch one copy over the whole stroke
- [x] A live preview strip in the panel, because the difference between
      spacing 4 and spacing 40 is obvious as a picture and meaningless as a
      number

**Staying responsive, which was the explicit requirement**
- [x] **Arc lengths are measured once** into a cumulative table; each stamp
      finds its place by binary search. The obvious implementation asks the
      path for the point at each fraction, which re-measures every segment for
      every stamp — `O(stamps x segments)`, and exactly how this kind of
      feature freezes a window. A test asserts that quadrupling the stroke
      does not quadruple-squared the time.
- [x] **The budget widens the spacing rather than refusing or truncating.** A
      10 000-unit stroke at 0.01 spacing asks for a million stamps; it gets
      4 000, spread over the whole stroke, and the caller is told the spacing
      moved. The user keeps their whole stroke at the density the machine can
      carry.
- [x] **The live preview has its own, much smaller budget.** It runs on every
      pointer move, so it is where a hang would actually appear.
- [x] The preview is painted by the *artwork* renderer in the real colour,
      not sketched as chrome — for a brush, the preview is the result.

**Build-up paint** — overlapping opacities *add*

- [x] A brush stroke at alpha 0.2 crossing one at 0.3 gives exactly **0.5** in
      the overlap, rather than the 0.44 ordinary source-over produces. Working
      over an area deepens it the way ink does, and repeated strokes accumulate
      in equal steps.
- [x] `PaintBlend` on the shape, saved with the document (format version 4);
      off by default, since Animate composites normally and a document whose
      overlaps silently deepened would surprise anyone who did not ask for it
- **The isolation group is the whole trick.** Additive compositing sums the
  source with the destination, and the destination includes the stage: applied
  straight to the canvas, dark paint at alpha 0.2 on a white background sums to
  white and the stroke *disappears*. So a layer holding build-up paint is drawn
  into its own transparent group, where the sum starts from nothing, and that
  group composites over the stage normally. The layer is the accumulation
  surface — build-up strokes deepen against their own layer and composite
  normally onto the layers below, exactly as a paint program's layer does.
- **Where colours differ the result is their mix**, weighted by how much of
  each is present: the sum happens in premultiplied space and is divided back
  out by the summed alpha. So build-up deepens paint rather than washing it
  towards white, which is what a naive additive blend would do.
- **Cost:** Vello sets blending per *layer*, not per fill, so each additive
  shape costs a layer push and pop. That is why it is opt-in rather than the
  default path. 300 build-up shapes render and read back in **1.4 ms**.

**Proved on the GPU, not on paper.** `headless_build_up.rs` renders through the
same path the window uses and reads the pixels back. Alpha is measured the way
the eye would — black paint on a white stage, `a = 1 - v/255` — and asserted
from the frame's *histogram* rather than named pixel coordinates, so a mistake
in reproducing the camera's mapping cannot masquerade as a compositing bug.

| Overlap of 0.2 and 0.3 | Measured |
|---|---|
| Build-up | **0.502** |
| Normal | **0.444** |

Further tests pin down that build-up paint does not dissolve into a light
background, that a normal shape on a build-up layer still composites normally,
and that build-up does not reach across a layer boundary. The same figures were
then read off a screenshot of the running application, from a document loaded
from disk — which also confirms the blend survives a save.

**Measured, release build, on the 14700K:**

| What | Time |
|---|---|
| One preview frame, 6 000-sample pattern stroke at 0.5 spacing | **0.57 ms** |
| Committing a 40 000-unit pattern stroke at 0.25 spacing | **3.5 ms** |
| 120 pattern strokes with a live preview on every move | **237 ms** |

**Verified on screen:** a fluid stroke drawn slowly then quickly comes out
tapered and visibly thinner where it sped up; a dot pattern stamps evenly
along a wave; thirty leaf-pattern strokes drawn over each other leave the
window at **56 fps**.


### ✅ Phase 5 — Importers

**CP-5.1 — `.fla` and `.xfl`**
- [x] Unzips `.fla`, reads `.xfl` folders, parses `DOMDocument.xml` and
      `LIBRARY/*.xml`, decodes Animate's edge format
- [x] Legacy CS4-and-earlier `.fla` (OLE2) detected and refused with a message
      that says how to convert it, rather than failing as "not a zip"
- [x] **Layer folders restored.** `parentLayerIndex` becomes `layer.parent` —
      but *only* when it points at a folder. Animate overloads the attribute:
      a masked layer points at its mask with the same one. Honouring that
      would nest a layer inside its own mask and break the positional rule
      that actually resolves masking.
- [x] **Bug found and fixed — imported masks clipped nothing.** `layerType`
      values `masked` and `guided` were being mapped to `Normal`. The
      positional mask rule reads the *kind*, so every imported mask silently
      claimed an empty run. Found by a test written for the folder work, which
      asserted the mask still claimed the layer beneath it and did not.
- [x] **Bug found and fixed — every import was grey.** Shapes were given a
      flat `#999999` regardless of the file: the `<fills>` and `<strokes>`
      tables were never read, only the presence of a `fillStyle` attribute.
      XFL declares styles once per shape and has each edge reference them by
      index, so the tables are now accumulated and resolved. Gradients average
      to their nearest flat colour and say so.

**CP-5.1b — merging into an open document**
- [x] `Scene::merge`, with `ImportTarget::Stage` or `::Library` matching
      Animate's two menu commands
- [x] **Every id is reallocated on the way in.** Both documents allocated from
      zero, so their id spaces overlap completely; copying without renumbering
      would repoint instances at whatever local symbol shared the number.
      Symbol ids are allocated *before* any artwork is copied, because an
      instance inside symbol A may refer to symbol B.
- [x] Layer `parent` links and nested instances are rewritten to match
- [x] A dangling reference is **left dangling** rather than repointed at an
      unrelated local symbol that happens to share the number
- [x] Names are the user's, so a clash renames the incoming symbol and
      **reports it**; resolved one at a time, so two incoming symbols wanting
      one name cannot collide with each other either
- [x] The source scene is never modified, so a failed merge cannot corrupt it
- [x] One undo step for the whole import, however many symbols it brings

**CP-5.2 — `.pdf` and `.ai`**
- [x] `buzz-import-pdf` on `lopdf` — MIT, pure Rust. Chosen over
      `pdfium-render`, which binds a multi-megabyte C++ blob that has to ship
      alongside the executable, for an API built around *rasterising* pages.
      We want the paths, in `f64`, still editable.
- [x] Path construction and painting operators, the graphics state stack,
      `cm` transforms, Gray/RGB/CMYK colour, `ExtGState` alpha
- [x] **Form XObjects are followed**, bounded at 12 levels. Illustrator wraps
      most artwork in them, so not following them would import many files as
      blank.
- [x] **MediaBox is inherited through the page tree**, not read off the page
      alone — real documents put it on the `Pages` node and would otherwise
      import at the wrong size
- [x] **The page is flipped.** PDF measures y upwards from the bottom-left,
      the stage downwards from the top-left; without the flip every import
      arrives mirrored. Tested by asserting a shape at the bottom of the page
      lands at the bottom of the stage.
- [x] Each page becomes a keyframe, so a multi-page document can be stepped
      through rather than stacked on itself
- [x] `.ai` v9+ is PDF internally, so one parser covers both; pre-v9
      PostScript is detected by its `%!PS` banner and refused with the fix
- [x] `n` (end path without painting) draws nothing — emitting it would fill
      every clipping rectangle in the document with black

**CP-5.3 — `.swf`**
- [x] `buzz-import-swf` on Ruffle's `swf` crate — MIT OR Apache-2.0, and the
      same project whose AVM2 Phase 8 plans to embed. **`swf-parser` was
      rejected: it is AGPL-3.0**, which this project cannot take.
- [x] `DefineShape` → Graphic symbol · `DefineSprite` → MovieClip with its own
      timeline · `PlaceObject` → an instance · `ShowFrame` → the next frame
- [x] **One layer per depth.** SWF's display list is depth-ordered and holds
      one object per depth, which is exactly what a layer is; flattening would
      lose the stacking order the movie depends on. Depth order is reversed on
      the way in, because SWF paints low depths first and our stack is
      front-first.
- [x] Moving an object becomes a keyframe; removing one ends the span
- [x] **Shape records are stitched back into paths.** An SWF shape is not a
      list of paths but a soup of edges, each naming the fill on its left
      (`fill_style_1`) *and* its right (`fill_style_0`). Edges are bucketed per
      style and chained by endpoint into closed loops; a `fill_style_0` edge is
      walked **backwards**, because reversing an edge swaps which side its fill
      is on. Getting that wrong yields shapes that look almost right with holes
      in the wrong places, so it is asserted rather than eyeballed.
- [x] Twips convert exactly: one twip survives as 0.05px, tested

**Exit test.** `crates/buzz-app/tests/import_round_trip.rs` runs the whole
path for all three formats: read the file, merge into a document that already
has artwork *and a colliding symbol name*, then save and reopen. It asserts
what only the seams can break — that every object id is still unique across
the stage and every symbol, that every instance points at a symbol that
exists, that names stay distinct, and that nothing is lost on save. It also
covers importing the same file twice, importing to the library leaving the
stage untouched, and a failed import leaving the document byte-identical.

**Honest limit.** Phase 5's original exit criterion was a frame-by-frame
comparison of an imported `.fla` against a render from Adobe Animate. **That
was not done and could not be**: it needs a licensed copy of Animate and a
reference file it produced, and a fabricated one would prove nothing. What is
verified is structural fidelity against files whose intended content is known
exactly, plus the disk round trip, plus on-screen inspection. The visual
comparison stays open as §7 item 21.

**Verified on screen, not just by test.** One fixture per format was written
(`buzz-app/tests/make_import_fixtures.rs`, `--ignored`), opened, and captured:
the `.fla` arrives with its blue background, its red symbol instance placed
and scaled, its layer folder intact and a purple tween span in the timeline;
the `.swf` arrives as two depth layers with keyframed motion and the removal
landing on the right frame; the `.pdf` arrives the right way up with its
Bézier still a Bézier. The grey-fill defect above was found this way, not by
a test — every test passed while every import was monochrome.

### ✅ Export — CP-6.1, images and sequences

The first thing to come *out*. `buzz-export` renders a frame offscreen at any
resolution and writes PNG; `File ▸ Export ▸ Export Image…` writes one, and
`Export PNG Sequence…` writes a numbered file per frame.

- [x] **One picture of the document, not two.** The walk that turns a `Scene`
      into Vello drawing commands moved out of the application into
      `buzz_render::document`, so the window, the exporter and the headless
      tests all encode a document through the same code. An exporter with its
      own copy eventually disagrees with the screen, and the user finds out
      after the render rather than while they are working.
- [x] **Chrome cannot leak in**, for the same reason: rulers, guides, selection
      handles, onion skins and now bones are drawn by the application in screen
      space and are not part of that walk. A test asserts an exported rigged
      frame contains no bone colour at all.
- [x] Any size up to what the GPU will render in one pass; beyond that it is
      **refused with the limit named**, rather than silently truncated
- [x] Transparent export, unpremultiplied on the way out — Vello composites in
      premultiplied alpha and PNG stores straight alpha, so a transparent file
      would otherwise come out with dark fringes
- [x] Sequences render on the GPU one frame at a time and **encode across every
      core**, in batches, so a 500-frame 1080p export never holds more than
      sixteen frames in memory
- [x] **It runs on its own thread with its own GPU context**, reporting
      progress and cancellable. A 500-frame export on the UI thread would stop
      the window dead with no way to tell a slow export from a hung one; a
      second context costs a few hundred milliseconds once. Stopping keeps what
      was already written.
- [x] Files are written to a temporary name and renamed into place, so an
      interrupted export cannot leave a half-written image
- [x] 8 GPU tests reading pixels back, plus 3 driving the whole job

**The bug the alignment test was written for.** A texture-to-buffer copy needs
rows on a 256-byte boundary, and a 550-pixel stage is 2 200 bytes — *not* a
multiple of 256. Getting the padding wrong shears the image progressively down
the frame, which looks like a rendering fault and is not one. The test asserts
the **bottom** row and the square's right edge on several rows, because the
first row would look perfect either way.

### ✅ Phase 7 — Rigging

**CP-7.1 — bones and armatures** (`buzz-rig`)
- [x] `Bone` and `Armature`: a tree of segments, each with a length, a rest
      angle and a pose angle **relative to its parent** — which is what makes a
      pose portable, and what lets a keyframe store an arm as three numbers
- [x] Forward kinematics, with `joints()` resolving the whole skeleton in one
      pass rather than walking to the root per bone
- [x] Built by dragging, as in Animate: each drag is a bone, and dragging from
      a bone's tip adds its child

**CP-7.2 — inverse kinematics**
- [x] **FABRIK** — reach backwards from the target, then forwards from the
      base, keeping every bone its own length. No Jacobian, no matrix
      inversion, and it settles a twelve-bone chain inside ten passes.
- [x] **Joint rotation limits**, imposed by reading the reached positions back
      as angles, clamping, and running forward kinematics again. Constraining
      inside FABRIK's own backward pass is the textbook variant and is only
      approximate; reading the angles out is exact, and forward kinematics then
      *guarantees* bone lengths — a rig that stretches looks broken in a way
      that a rig which merely fails to reach does not.
- [x] Limits clamp **across the wrap point**, so a joint near ±π takes the
      nearer end of its range instead of swinging the long way round
- [x] **Pins**: a solve climbs no further than a pinned joint, which is what
      holds a foot on the ground while the hips move
- [x] An unreachable target lays the chain straight towards it **in one pass**
      rather than burning the whole iteration budget every frame of a drag
- [x] The root does not move: dragging a hand must not slide the character
      across the stage

**CP-7.3 — puppet warp**
- [x] Moving Least Squares with a **similarity** fit — rotation, uniform scale
      and translation, no shear. In two dimensions a similarity is exactly
      multiplication by one complex number, so the least-squares fit has a
      closed form and needs no matrix inverse. Fitting an affine instead lets
      artwork shear unevenly, which reads as melting rather than posing.
- [x] Tests pin the properties that matter: moving every handle equally moves
      the artwork rigidly, rotating every handle rotates it **without changing
      its size**, and one dragged handle is a pure translation

**CP-7.4 — skinning**
- [x] Every point of a path — anchors *and* control points — is weighted across
      the four nearest bones at bind time, so artwork bends at a joint instead
      of tearing. Weighting only the anchors would drag the ends of a segment
      while its handles stayed behind, and the curve would fold through itself.
- [x] **Weights are bound against the rest pose and stored**, not recomputed
      from the posed skeleton — recomputed weights would change as the
      character moves, and artwork would visibly pop from one bone's influence
      to another mid-animation
- [x] Rigid attachment as well, for a chain of symbols: a forearm drawn as its
      own symbol should turn about the elbow, not deform

**In the document**
- [x] `ObjectKind::Armature` and `ObjectKind::Warp`, so keyframes hold rigs the
      way they hold anything else — and **poses tween**: two keyframes, and
      every frame between them interpolated joint by joint, each turning the
      shortest way round
- [x] A shape tween over a rig interpolates the **skeleton**, not the outlines;
      blending two deformed outlines would make shapes no pose could produce
- [x] Format version 6 stores **only the pose** — the angles — never the
      deformed artwork it produces, because a saved deformation is a second
      copy of something the file can already work out, and the two would drift
      apart the first time a bone was edited
- [x] A file describing a **cyclic skeleton** loads with the cycle broken
      rather than hanging the solver
- [x] Hit-testing tests the *posed* geometry: a bent arm is clickable where it
      is drawn, not along the straight one it started as

**The tools**
- [x] Bone tool (M) — drag across artwork to rig it, drag from a tip to add a
      bone, drag a bone to pose it with IK. Asset Warp tool (W) — touch artwork
      to give it a grid of handles, then drag them.
- [x] Bones drawn as Animate draws them: a tapered quadrilateral, widest a
      quarter of the way along, so the direction of a bone reads at a glance
- [x] The Armature panel lists every bone with its parent, its pin, its joint
      limits in **degrees**, and Reset Pose / Set Rest Pose
- [x] One drag is one undo step, and each panel change is its own

**Deliberate deviation — an armature is an object, not a layer.** Animate moves
rigged artwork onto an *armature layer* whose keyframes are poses. Here an
armature is an `ObjectKind` on an ordinary layer. What that buys is everything
the document already does — keyframes, tweens, undo, grouping, the library,
symbol nesting, importing and id remapping all work on it with no second code
path. What it costs is that the timeline does not mark a rigged layer as an
armature layer and nothing stops you drawing on it (§7 item 33).

**Three defects found by building the exit test and driving the real window:**

1. **Edits landed on the wrong keyframe.** `update_object` searched keyframes
   in order and took the first match — but F6 duplicates a keyframe by cloning
   the `Arc` around its objects, so *one id legitimately appears on several
   keyframes*. Posing a rig on frame 12 therefore changed frame 0 and appeared
   to do nothing. This was never rigging-specific: dragging or transforming any
   shape on a later keyframe had the same fault. `Scene::update_object_at` now
   targets the keyframe owning the playhead, and every editor edit goes through
   it.
2. **Snapping moved the click before the rig could read it.** A bone lies
   *inside* the artwork it drives, and snap-to-objects pulls a click towards
   the nearest edge — so clicking a bone near the edge of a limb jumped the
   click onto that edge and missed. Rig gestures now use the raw pointer.
   Found by tracing what the gesture actually saw, after three attempts to pose
   a bone did nothing.
3. **The tool strip clipped.** With the Actions panel open the left strip lost
   everything below the Brush — including the Bone and Asset Warp tools it had
   just gained. It scrolls now. Only a picture was ever going to show this.

**A limitation found and fixed, not papered over.** A warp moves *points*, and
a rectangle has four: dragging a handle in the middle of a straight edge moved
nothing at all, which reads as the tool being broken. Artwork is now subdivided
exactly — each piece a sub-segment of the original curve, so the shape before
any drag is geometrically identical — when it becomes warpable.

**Verified on screen, not just by test.** The window was driven with real mouse
input: the Bone tool dragged across a limb, a second bone dragged from the
first one's tip, and the arm posed by dragging a bone — the artwork bends with
the skeleton, the Armature panel shows both bones with their pin and limit
controls, and `File ▸ Export ▸ Export Image…` opens with the stage's size
filled in. The trace line in `begin_rig_gesture` was added while chasing
defect 2 and left in place, because what a rigging gesture found is exactly
the thing that is invisible when it goes wrong.

### ✅ Masking

Animate's six layer types have been in the model since CP-1.2, and
`mask_groups()` has resolved the positional rule — *a mask claims the unbroken
run of masked layers below it* — since then. **Nothing ever called it.** For
five phases a mask layer drew as ordinary artwork and every test passed,
because the rule was tested and the *clipping* was never rendered.

- [x] Masked layers are clipped to the mask layer's artwork, in the shared
      draw walk, so the window, the exporter and the headless tests all agree
- [x] The mask's own artwork is not drawn — it is a stencil, which is what
      Animate does with it
- [x] **Non-zero fill**, so a mask made of three separate blobs shows through
      all three; even-odd would punch a hole wherever two overlapped
- [x] Masks inside symbols clip that symbol's layers, wherever the instance is
      placed
- [x] The clip goes through the same document-space clipping and rebasing as
      artwork, so a mask survives extreme zoom like everything else

**Deliberately Animate's rule, not the obvious one.** On the stage a mask
clips only once its layer is **locked** — Animate's own behaviour, because you
cannot draw inside a region you cannot see. An export always clips. That is one
flag, `MaskDisplay`, set differently by the two callers, rather than a
difference nobody wrote down.

**Verified by pixels**, in `buzz-export`'s headless tests: a full-stage
rectangle under a porthole mask shows only inside the porthole, a two-blob mask
shows through both blobs and not between them, and a mask inside a symbol clips
its instance.

### ✅ Sound, and automatic lip sync

An animator works to a soundtrack: the dialogue arrives first and every
decision after it is made against what they can hear. So this is not a
publishing feature — it has to be audible *while drawing*, at the right frame,
from wherever in the document the work is happening.

**`buzz-audio`** — decoding, waveforms, playback and analysis, with no
knowledge of documents or layers
- [x] WAV through `hound` (Apache-2.0/MIT) and everything else — MP3, OGG,
      FLAC, AAC — through Symphonia (**MPL-2.0**, file-level copyleft, which
      permits use as a library; noted because this project refused an AGPL
      dependency in Phase 5 and the distinction matters)
- [x] Two readers rather than one on purpose: WAV is what dialogue arrives as,
      and when a file will not open it matters which reader said no
- [x] Bit depth is scaled by its **own** range — a 24-bit file scaled as
      16-bit comes back 256 times too loud, which is not subtle but is easy to
      write
- [x] Decoded once into memory, so seeking is arithmetic rather than a decoder
      re-syncing to a bitstream on every playhead move
- [x] Waveform **peaks**, not averages: an averaged waveform of speech is a
      featureless blur, because the halves cancel

**Playback** (`cpal`, Apache-2.0)
- [x] The device is opened **lazily** and kept: a document with no sound never
      touches the audio device, and one that does opens it once
- [x] **The audio clock is authoritative while playing.** The playhead is told
      where the sound has reached, not the other way round — otherwise every
      dropped frame nudges the dialogue, and lip sync drifting out over a long
      take is the one defect an audience always notices.
- [x] Every sample's source index is computed from its absolute position, so
      playback cannot accumulate drift however many buffers go by
- [x] A machine with no working audio records why, once, and the editor carries
      on silently rather than refusing to run

**In the document** (format version 7)
- [x] Sounds live in the library; the audio itself goes in the container's
      `media/` directory — **reserved for exactly this since Phase 1** — stored
      uncompressed, because MP3 does not deflate twice and an unzip should
      recover a playable file
- [x] A sound is attached to a **keyframe**, with Animate's four sync modes
      (Event, Start, Stop, Stream), so one layer can carry a whole scene
- [x] F6 does **not** duplicate a keyframe's sound: on a dialogue layer that
      would start the take again on top of the one already playing
- [x] A sound whose media entry is missing keeps its name, its duration and
      every keyframe that refers to it, and plays silence — dropping it would
      delete the user's edits along with it
- [x] Importing a document brings its sounds and renumbers them, like symbols

**The rule the whole design turns on — audio belongs to the document**

`Scene::stage_cues` reads the **document's own timeline**, ignoring which
symbol is open for editing, at any depth. So dialogue on the root keeps
playing when you step into a character to animate its walk, and keeps playing
when you step from there into its head to animate the mouth. There is no code
path that could get this wrong, because there is no code path that knows what
symbol you are in — it is the same distinction saving already makes.

**Automatic lip sync**
- [x] Preston Blair's ten mouth shapes, in Animate's order, so a mouth symbol
      drawn for Animate works here unchanged: the viseme *is* the frame of the
      symbol, shown by an instance in single-frame mode
- [x] One keyframe per **change** of shape, not one per frame — a keyframe on
      every frame is unreadable in a timeline and impossible to adjust
- [x] The lips close in the silence *before* a word, as anticipation, rather
      than on the first voiced frame, which would delay every word
- [x] Shapes are held for at least two frames: below that the eye reads a mouth
      as vibrating rather than speaking
- [x] Loudness is judged against the clip's own peak, so quietly recorded
      dialogue still animates
- [x] The whole run is one undo step
- [x] A mouth symbol without a frame per shape is refused **with both
      numbers**, not silently animated to the wrong shapes
- [x] `New Mouth Symbol` makes a ten-frame placeholder to draw over, so the
      timing can be reviewed before any drawing is done

**What the analysis is, stated plainly.** Animate runs a trained phoneme
recogniser. There is none here, and pretending otherwise would be worse than
useless — a bad recogniser makes a character mouth nonsense, which the animator
has to find by watching. What this does is signal analysis: loudness, where the
energy sits in the spectrum, and how noisy the waveform is, mapped to the
shapes those three things can actually distinguish — silence from speech, open
vowels from rounded ones, fricatives from vowels. It does **not** tell `p` from
`b` from `m`; those differ in ways an amplitude spectrum cannot see, and they
land on the closed shape, which is where an animator would put them anyway. The
timing is right and the openness is roughly right, and the timing is the
tedious part. The FFT is forty lines here rather than a dependency, and is
tested against a signal whose spectrum is known.

**A defect found by driving the window, not by a test.** F8, Ctrl+E and
Ctrl+F8 — Convert to Symbol, Edit Symbol, New Symbol, *the* keys of a symbol
workflow — did nothing from the keyboard. They were in the command map, printed
beside their menu items, and the list that actually reads the keyboard had
never included them; nor had Cut, Copy, Paste, Close or Quit. All are bound
now, and `every_shortcut_is_reachable_from_the_keyboard` fails the build if a
command ever advertises a shortcut nothing binds.

**Two more, found the same way.** A document opened with sound in it drew no
waveform, because sounds were only decoded when playback or lip sync asked for
them — the editor now refreshes each frame, which costs a revision comparison.
And the waveform was drawn *under* the frame cells, which paint their own
backgrounds over it.

**Verified on screen.** A fixture document with dialogue on the root, a
character containing a head, and a locked porthole mask: the mask clips the sky
to the porthole on the stage, the waveform draws across the Audio layer's
frames with the syllables visible in it, and — stepped inside the Character
symbol — the Lip Sync dialog names the soundtrack it is about to use,
*"Dialogue — 1.8s, from frame 0 · From the main timeline, wherever you are
editing"*. Syncing from in there wrote **13 keyframes over 44 frames (17
silent)** onto the symbol's own layer. Playback was confirmed against the real
audio device: the stream opens, and after 700 ms the position has reached frame
17 at 24 fps.

### ✅ Scripting — CP-8.1, and the Actions panel

Taken out of order, ahead of Phase 6, because it is the feature that lets a
user do the things nobody has built a button for yet.

**The engine** (`buzz-script`)
- [x] QuickJS through `rquickjs` — MIT, pure Rust, no bundled binary blob
- [x] Animate's own vocabulary: `fl.getDocumentDOM()`, `fl.trace()`,
      `document.addNewRectangle({left, top, right, bottom})`,
      `document.getTimeline()`, `timeline.layers[i].name`,
      `document.convertToSymbol('graphic', name)`, `document.library.items`
- [x] Rectangles, ovals, fill and stroke colour, selection, moving and deleting
      artwork, layers (add, delete, rename, show/hide, lock, **depth**),
      frames (insert frame, keyframe, blank keyframe), the playhead, document
      size, frame rate and background, and the library
- [x] **A script cannot hang the application.** An interrupt handler with a
      deadline stops `while (true) {}` with an error; a memory ceiling and a
      stack limit fail a runaway allocation or an infinite recursion as script
      errors rather than as a dead process.
- [x] **A script cannot leave the editor.** `require`, `fetch`, `process`,
      `open` and `XMLHttpRequest` do not exist, and a test asserts each is
      still `undefined`.
- [x] Errors are worded for a person and carry the line number
- [x] 24 tests

**Two decisions worth the space**

- **Rust exposes flat primitives; a JavaScript prelude shapes the API.** The
  Rust side stays plain functions with no property accessors or prototypes,
  and the part that has to match Animate — names, argument shapes, the
  `{left, top, right, bottom}` rectangle — is written in readable JavaScript
  that can be diffed against Adobe's own JSFL documentation.
- **Scripts mutate a working copy rather than submitting to a command queue,**
  which is what CP-8.1 originally said. A queue cannot answer a read *after* a
  write, and `d.addNewRectangle(); d.selection.length` is most of what a script
  does. The one property the queue was for — one run, one undo step — comes
  from committing that working copy in a single `Document::edit` instead.

**One run is one undo step, and a failure keeps its work**
- [x] Forty scripted rectangles are one Ctrl+Z; a test proves it
- [x] `end_gesture` follows each run, so two runs are two steps rather than
      coalescing the way the moves of one drag deliberately do
- [x] A script that fails half way keeps what it drew and reports the error
      next to it. Discarding an hour of generated artwork because the last line
      had a typo would be indefensible.
- [x] Reading the document is not editing it: a script that only traces leaves
      the document clean, so inspecting a file cannot mark it dirty
- [x] The editor adopts the selection and playhead the script left, so
      `d.selectAll()` and `t.currentFrame = 3` do something visible
- [x] A script run inside a symbol edits **that symbol** — the same rule every
      tool and panel already follows

**The Actions panel** (CP-8.2, first cut)
- [x] F9 opens it, as in Animate; **Commands** joins the menu bar (§8.1)
- [x] Script on the left, Output on the right — the panel is wide and short, and
      side by side the error appears level with the code that caused it
- [x] Ctrl+Enter runs. It is consumed **before the panels are drawn** and
      **while a text field has focus**, which every other shortcut deliberately
      is not: the moment Ctrl+Enter is wanted is precisely when the caret is in
      the code box, and consuming it early stops the editor inserting a newline
      as well as running.
- [x] Five built-in examples — describe the document, a grid, a ring of dots,
      a parallax depth stack, convert the selection to a symbol
- [x] **The examples live in Rust and a test runs every one of them.** Sample
      code that no longer works against the API is worse than none: the user
      takes it for the reference and blames their own typing.
- [x] `--script <file>` runs a script at startup with the panel left open,
      which is Animate's command-line JSFL and also how this was checked

**Bug found and fixed on the way — `--gpu NVIDIA` tried to open a file called
NVIDIA.** The trailing-path argument was the first one not starting with `--`,
which is the *value* of a flag as often as it is a path. Flag values are now
skipped, and `--script` would have inherited the same defect.

**Verified on screen, not just by test.** The window was captured with a script
run over it: forty shapes and a scripted layer on the stage, the four traced
lines in Output, the Commands menu in the bar, and "Script finished in 1 ms" in
the status bar.

**Bug found by that screenshot — an empty box for a triangle.** "Examples ▼"
drew as "Examples □". §4 has recorded since Phase 4 that `▼` is a glyph egui's
bundled fonts have, **and that is wrong**: a probe row rendered
`▼ ▾ ▸ ⏷ ⌄ ↓ ▽ ⯆ ⬇ ➤ ▲` and only `⏷` and `⬇` came out (`▶` renders, which is
what made the claim look right). The Library panel had been drawing an expanded
folder as an empty box for the same reason since Phase 4, and now uses `⏷`.
No test can see this; only a picture can.
### ✅ Animate's frame commands, and importing an Animate asset library

**The frame family, completed.** F5, Shift+F5, F6, F7 and Shift+F6 have been
here since Phase 3. The rest of Animate's timeline commands now are too, on
Animate's own keys:

| | |
|---|---|
| Cut Frames | Ctrl+Alt+X |
| Copy Frames | Ctrl+Alt+C |
| Paste Frames | Ctrl+Alt+V |
| Clear Frames | Alt+Backspace |
| Reverse Frames | — |

**Clear Frames is not Clear Keyframe**, and the difference is worth stating:
Clear Keyframe removes the keyframe and hands its frames back to the one
before; Clear Frames keeps the keyframe and empties it. Paste Frames makes a
keyframe where the playhead is before pasting — pasting into the middle of a
span would otherwise change the artwork from wherever that span began — and
gives the arriving objects fresh ids, so pasting twice gives two drawings
rather than one shared between two frames. Reverse Frames swaps the *contents*
of a layer's keyframes end for end and leaves their timing alone, which is what
makes it useful on a cycle.

**A test caught the wiring, not a person.** The new shortcuts were in the
command table and not in the list that actually binds keys, so they would have
been printed in the menu and done nothing when pressed — exactly the defect
`every_shortcut_is_reachable_from_the_keyboard` was written for after F8 and
Ctrl+E did it in Phase 4.

### ✅ Bringing an Animate asset library across

An animator moving here has a library of assets in Animate, and no reason to
rebuild it. `Documents/Adobe/Animate/<year>/Assets/Custom/<guid>/` holds one
folder per asset: a `manifest.json` with the name and how Animate files it, the
asset itself as a `.fla` — which is an XFL container this program has read
since Phase 5 — and thumbnails. So the import needed no new format work at
all; what was missing was the walk.

**Assets ▸ From Animate…** picks the folder (opening on this machine's own
Animate assets folder, newest year first, because the path is long and buried),
scans it, and imports everything on its own thread with a progress bar. A
library of a thousand is a thousand zip archives to open, which is a minute or
two — doing it on the UI thread would freeze the window for all of it.

- Each asset is filed as `Animate/<role>/<subCategory>`, under one folder of
  its own: Animate's arrangement kept, because that is the one they know, and
  separate, because dropping a thousand assets among somebody's own would be
  rude.
- **One bad file costs one asset.** Failures are named in a summary rather than
  stopping the run.
- **Bitmap assets are skipped rather than failed.** Animate's panel takes
  images as well as symbols and this program does not read bitmaps yet (§7 item
  22); three hundred "cannot import .png" lines would bury the real problems.

**Measured against the real library on this machine**: 1162 assets found in the
Animate 2024 folder, 32 of them bitmaps, and 24 of 25 sampled imported cleanly
— the twenty-fifth being one of those bitmaps.

### ✅ Three fixes: Open takes Animate files, the stage scrolls, docks resize

**File ▸ Open only offered `.buzz`.** The importers have been there since
Phase 5, but only behind File ▸ Import — so the program refused an Animate
document, which is the very file somebody coming from Animate reaches for
first. Open now lists everything it can read: `.buzz`, `.fla`, `.xfl`, `.swf`,
`.pdf`, `.ai`. A foreign file opens as a **new, untitled** document: what comes
back is a translation, however good, and Save must ask for a `.buzz` file
rather than write back over an Animate source this program cannot produce. What
did not survive the translation is reported in the same summary window an
import uses. Verified by opening a `.fla` through the dialog: 640×480 stage,
both shapes, fitted to the window.

**The stage had no scrollbars.** Panning was space-drag, middle-drag or the
Hand tool, and none of those tells you *where you are*: with the view off the
pasteboard there was nothing on screen to say which way the artwork lay. There
is now a bar along the bottom and one down the right. The thumb's length is how
much of the work is on screen and its position is where — dragged or clicked,
either moves the view. They scroll over the stage plus everything drawn on this
frame plus a stage's worth of margin, so the extent grows with the drawing
rather than being a fixed canvas the way Animate's is. The thumb never shrinks
below a grabbable size, which matters here more than in most programs: at a
trillion per cent the honest proportion is a fraction of a pixel.

**The tool strip was wider than the tools.** The left dock opened at 58 points
for a 30-point button, leaving half a column of empty grey beside every tool.
It opens at 46 now, and the strip **flows into as many columns as it is given
room for** — one is Animate's, and widening the dock gives two or three rather
than more empty space.

**There was no way to say how long the film is.** F5 and Shift+F5 add and
remove a frame on one layer, which is the right tool inside a scene and a poor
one for "make this shot four seconds". The transport now reads `Frame 1 of
[40]`, and that number is the document's length: dragging it extends every
layer, or trims them from the end. It is one undo step per drag, not one per
frame passed through, and the playhead is brought back if the document was
shortened underneath it. Verified on screen: 1 frame to 40, the span redrawn to
match and the readout going to "1.67 s".

**Dragging a dock's edge did nothing.** `Panel::resizable(true)` puts its handle
on the panel's edge and registers the interaction as the panel is drawn — but
the stage is a central panel drawn *after* every dock, and its own
click-and-drag covers the whole area, so it took the pixels the handle needed.
What that looks like from the outside is a panel that resizes and springs back,
because the live drag is panning the stage underneath instead.

The fix moves the authority: the docks are laid out at **exactly** the width the
workspace says, and the boundaries are our own splitters, drawn last so nothing
can take them. They move the workspace's numbers, which are what gets saved —
one source of truth instead of egui's stored size and ours disagreeing. Proved
by dragging the right dock from x=1020 to x=806 and finding it still there.

### ✅ The program looks like somebody's — icon, banner, About, launcher

**A launcher.** `BuzzAnimate.bat` in the repository root builds if the sources
have changed — a no-op once warm — and starts the editor, passing a document
path, `--gpu`, `--script` or `--dev` straight through. It refuses helpfully
rather than flashing a console and vanishing: no cargo, a failed build or a
missing binary each print a line and wait. `Create Desktop Shortcut.bat` puts a
shortcut on the desktop pointing at the *launcher*, so it survives a rebuild, a
`cargo clean` and switching between the release and debug builds.

**A found bug:** the first version launched through `start`, and the editor
came up minimised at –32000, –32000 — `start` hands the child whatever window
state it was itself given, and from a script with no console of its own that is
"minimised". It runs the binary directly now. Only launching it and looking for
the window would ever have caught that.

**An icon, from the studio's own artwork.** The character's head, cut from the
Khayal 3 Baje thumbnail and set on the show's orange: the lettering runs right
up against the figure, so painting it out takes a bite out of the artwork with
it, and the head is both clean and the part that still reads at sixteen pixels.
Seven sizes plus a hand-written `.ico` holding six of them — a taskbar wants the
small sizes crisp rather than downsampled from 512.

- The **window** icon is decoded from the PNG at startup.
- The **taskbar** icon is a *second* icon, set separately — see below.
- The **executable** carries it too, through a build script. That script never
  fails the build: embedding needs a resource compiler, and a machine without
  one should still get a working editor.

**The mark was drawn, and then put back.** An attempt at a program mark — a play
triangle with the onion-skin ghosts of the frames behind it, on the banner's
orange-through-blue tile — was drawn on the argument that the character's head
is a *show's* mark and says nothing about what the application does. The
studio's answer was the character, and the character it is: whose program this
is was the point. Both drawings are kept, and the icon set is *generated* rather
than hand-edited: `tools/make-icon.ps1` writes every PNG and the `.ico` from the
artwork, and `tools/make-icon-alternate-mark.ps1` is the abstract mark, there if
it is ever wanted. Anyone changing the icon edits a script and re-runs it, which
is also the only way the seven sizes stay in step with each other.

**And it is lettered BA.** The head sits in the top seven-tenths with a darker
band of the same orange under it carrying the initials in white. The lettering
is why every size is now **composed at its own size** rather than reduced from
one master: two characters shrunk from 512 pixels to 32 are a grey smudge, while
the same two drawn at 32 keep their stems. The letters are also fitted by
measurement rather than by a chosen point size — "BA" in a heavy face is wider
than it is tall, and fitting it by height alone ran the A off the tile.

**Sixteen pixels is the letters alone.** A head and a caption at eight pixels
each are two smudges; BA by itself still reads. That size goes in the `.ico`,
for file lists and small views. Arial rather than Arial Black there, since the
heavy face is too wide to fit two letters across sixteen pixels at all.

**Windows keeps two icons per window, and we were setting one.** The title bar
draws the *small* icon and the taskbar draws the *big* one, and winit's
`with_window_icon` sets only the small; the big one stayed null, so the taskbar
fell back to its blank-sheet-of-paper placeholder while the title bar showed the
logo perfectly. `with_taskbar_icon` — Windows-only, hence the `cfg` — now sets
it. Each is fed the drawing nearest its own size: 32 for the title bar (24 at
this machine's scaling), 128 for the taskbar and Alt+Tab.
`the_window_carries_a_small_icon_and_a_large_one` guards both, because the
failure is silent and shows up only on a strip of screen the application does
not draw.

**The process says who it is.** `SetCurrentProcessExplicitAppUserModelID` names
it `BuzzcafMedia.BuzzAnimate`. Without that, a window launched through a batch
file and a command prompt is filed under whatever ran it; the same identity is
what a pinned button and a running one match on.

**The taskbar draws the *executable's* icon, and the shell caches it.** This is
the part worth writing down, because it cost an hour. Both window icons can be
correct — read back from the live window with `WM_GETICON` and photographed —
and the button still shows the placeholder, because the taskbar resolves the
icon through the shell, and the shell's cache is keyed on the executable. Every
rebuild invalidates it and it re-caches as *nothing*. `ie4uinit.exe -show`
sometimes clears it; restarting Explorer always does. It is a developer's
problem, not a user's — their executable does not change every four minutes —
but anyone editing the icon here will see a blank sheet of paper and think the
change failed.

**Help ▸ About** shows the Spilled Coffee Studios banner, the version — the
first thing anybody reporting a problem is asked — and what this is built on.
The banner is uploaded the first time the window opens rather than at startup,
because a window nobody opens should not cost a texture.

**A brand band across the top of the window**: orange, through grey, to blue.
It was a frame round all four edges first, and that read as a *highlight* — the
shape a program uses to say "this window has focus" or "this thing is
selected". A band along the top is a masthead instead, and nothing at the edge
of the stage competes with the artwork. A mesh rather than rectangles, because
a gradient needs a colour per vertex; tests hold it to its three colours, to a
smooth ramp with no visible banding, and to painting without panicking at any
window size.

### ✅ Crash recovery — autosave offered back, and a pause that saves

Autosave has existed since Phase 1: a recovery copy written **beside** the
document rather than over it, atomically, on the background pool, and discarded
when the document is properly saved. Three things were missing, and without them
it was an autosave nobody would ever see.

**1. Nothing offered it back.** `find_recoveries` existed and had no caller. On
launch the program now scans its own recovery directory and every directory a
document has been opened from or saved to — remembered in the workspace, capped
at eight, because autosave writes beside the document and a fresh launch has no
other way to know where that was. What it finds is listed in a prompt: what each
one is, whether it was ever saved at all, and how long ago it was written
("never saved · 4 minutes ago"), with **Recover**, **Discard** and **Later**.
It never opens one by itself — the user may have closed without saving on
purpose, and replacing a document with a copy of unsaved changes is its own kind
of data loss.

**2. Unsaved work went to the system temp directory**, which is swept by the
operating system and by every cleanup tool going. An hour of drawing that was
never saved is the work most worth keeping, and that was a poor place to keep
it. It now lives beside the workspace and the asset library, under
`%APPDATA%/BuzzAnimate/recovery`.

**3. A crash still cost up to two minutes.** Two things were added:

- **A pause writes.** Five seconds of no change and the edit goes to disk. An
  animator draws in bursts, and the gap between two of them is when writing is
  free. It survives even a hard kill, where no code of ours runs at all.
- **A panic writes.** The current scene is kept in a global slot — a pointer
  copy per change, since a scene is a tree of `Arc`s — and a panic hook writes
  it before the process dies, printing where it went. The hook wraps whatever
  was there, so the backtrace is still produced: a crash still needs reporting,
  it just should not also cost the artwork.

**Two real bugs, found by crashing the program on purpose.**

- Every unsaved document was filed as `untitled.recovery.buzz` in one shared
  directory, so relaunching after a crash **overwrote the recovery while the
  prompt offering it was still on screen**. Unsaved work now gets one slot per
  session (`untitled-<process id>`), and the prompt shows that as "Untitled
  work" because the number means nothing to the reader.
- Recovering a file adopted it as the document's path, so its own autosave
  wrote `…recovery.recovery.buzz` and Save would have written back over the
  evidence. A recovered document is now **untitled again**: Save asks where to
  put it, and the file it came from is moved aside as `…recovered.buzz` rather
  than deleted or offered again.

**Verified by killing the process, not by asserting.** Drew a rectangle in a
fresh document, waited out the pause, terminated the process outright with no
chance to run any code, relaunched: the prompt appeared, Recover was pressed,
and the rectangle was back on the stage.

### ✅ A New Document dialog — Full HD, 24 fps, and remembered

New used to make Animate's default document: 550×400 at 24 fps, which was the
right answer in 2005 and is the wrong one now. Almost everything made today is
delivered at 1920×1080 or at a phone's proportions, and changing a document's
size after the artwork exists means rescaling every layer and every camera
move. **File ▸ New now asks**, and the answer costs one keypress.

- **Full HD at 24 fps** is the default: the size things are delivered at, and
  the rate animation is drawn on.
- Seven presets — Full HD, HD, 4K UHD, Square, Vertical, Film 2K, and Animate's
  own 550×400 for opening older work at its native size — each with what it is
  *for* on its tooltip, and the chosen one highlighted.
- Frame rate as a field plus one-click 12 / 24 / 25 / 30 / 60: on twos, film,
  the two broadcast rates, and games.
- The summary line names the ratio the way people say it: `1920 × 1080 · 16:9
  · 24 fps`.
- Enter creates, Escape cancels, and the window's own close button counts as
  cancelling.
- **Nothing changes until it is answered.** The document on screen is untouched
  while the dialog is up, which is what makes New safe to have on Ctrl+N.

**The settings are remembered**, kept with the workspace — a preference
belonging to the person rather than to any film, and one that must not travel
inside a `.buzz` file. Somebody making a series makes twenty documents at one
size; the second onwards is Enter. The *window itself* opens on that size too,
or the promise would only be half kept.

A new document also opens **clean**. `Document::mark_clean` was added for it:
building a scene bumps its revision, so a freshly made document used to report
unsaved changes from the moment it appeared — the asterisk in the title bar was
there before a single mark was made.

**A test was writing the user's own layout.** Saving a preference writes
`workspace.json`, and `Editor::default()` in a test saves like anything else —
so running the suite could leave the next launch opening at whatever size a
test had asked for. `workspace_path()` now honours `BUZZANIMATE_WORKSPACE`, the
test harness points it at a per-process temp file, and each test editor starts
from the default layout so one test cannot decide what the next one finds.

### ✅ Shape recognition, and zoom on the stage

**Draw roughly; get the shape you meant.** Animate recognises a hand-drawn
circle as a circle, four rough strokes as a rectangle and a shaky stroke as a
straight line. `buzz-geom::recognise` does the same:

- The path is flattened once, and each candidate is fitted and scored by the
  **worst** distance from a point to the ideal shape, as a fraction of the
  shape's size. Worst rather than average, because an average hides exactly the
  case that matters: a circle with one corner pulled out is not a circle, and
  its average error is tiny.
- Round is tried before square. A hand-drawn circle passes a loose rectangle
  test at its corners far more readily than a hand-drawn rectangle passes the
  circle test, so the other order squares off circles.
- A rectangle may be **at an angle** — the smallest-area orientation, coarsely
  searched then refined — and anything within three degrees of the page is
  snapped to it, because a rectangle returned at 1.4° reads as a mistake rather
  than as a drawing.
- Circles and squares are named as such when their two dimensions are close: it
  is the shape somebody drawing "a circle" meant, not an oval that happens to
  be nearly round.
- An **open** path is a line or it is nothing. Closing a drawn arc into an oval
  would invent a shape the hand did not make.
- **It never fails into a wrong answer.** A scribble, a star, an arc: `None`,
  and the artwork is left exactly as drawn, with the status bar saying so.
  Replacing a drawing with something the animator did not draw is worse than
  doing nothing.

Three tolerances — Strict, Normal, Tolerant — as Animate's Preferences offer,
with the same names. **Straighten recognises first**, as Animate's does: it is
the command reached for after drawing a rough circle, and easing the curve of
something that could have *been* a circle is a worse answer than the one that
was wanted. `Modify ▸ Shape ▸ Recognise Shape` asks for it directly.

**Zoom, where the eye already is.** The status bar has carried a zoom field
since Phase 2, and it is the last place anybody looks while drawing. There is
now a control in the **stage's own top-right corner**: zoom out, the percentage
(draggable, at a speed proportional to itself, so one gesture works at 50% and
at a trillion), zoom in, a presets menu with Animate's steps plus Fit in
Window / Show All / Show Frame, and a Hand toggle. The mouse could already do
most of this — the wheel zooms about the cursor, and space or the middle button
pans whatever the tool — but neither is discoverable, and a control in the
corner is.

**A missing glyph, caught by looking, for the third time.** The presets button
used `▾` (U+25BE), which egui's bundled fonts do not have; it drew as an empty
box in the first screenshot of the control. It is now `⏷`, and both it and
the `▾` that failed have been added to `theme::font_has`'s inventory — the
list of what to use, and the list of what never to reach for again.

**Verified on screen**: a deliberately wobbly ring drawn freehand with the
pencil, selected, and Modify ▸ Shape ▸ Recognise Shape — a clean circle, and
"Recognised a circle" in the status bar. The zoom control taken from 150% to
404.93% by its own button, and back to 134.83% by Fit in Window.

### ✅ A light interface

Animate offers a dark and a light interface; this now does too, from the Window
menu, and the choice is kept with the workspace — a preference belonging to the
person rather than to the film, so it never travels inside a `.buzz` file handed
to somebody else.

**One set of names, two answers.** `Palette` was twenty associated constants;
it is now twenty functions, each holding both values and returning the one for
the current theme. That is the whole design: no piece of chrome decides for
itself what "the panel colour" is, so no piece of chrome can be left behind in
the wrong theme. The current theme is a process-wide atomic rather than a handle
threaded through every painter — there is one window and the chrome is drawn on
one thread, and expressing that with a parameter on ninety call sites would be
ceremony rather than safety.

Three colours are deliberately the *same* in both:

- **The accent blue** and the selection colour. An accent that changes with the
  theme stops being one.
- **The pasteboard stays mid-grey.** Its job is to be clearly not the stage, and
  a white document on a white surround loses the edge of the frame — the one
  boundary an animator has to see at all times. Animate keeps its light theme's
  pasteboard grey for the same reason, and a test asserts a white stage cannot
  disappear into it in either theme.

**A bug found by looking, again.** The first switch left every panel dark while
the rulers, pasteboard and timeline went light: egui redraws only when
something asks it to, and the theme change is raised from *inside* the frame
being built, so the restyle landed at the top of a frame that never came. The
window now asks for one. Everything driven by the palette directly had already
changed, which is exactly why the half-changed window was so legible as a
symptom.

Tests cover both themes rather than whichever happens to be current: text
against panels, secondary text, ruler numbers against the ruler, the stage
against the pasteboard — plus one that the light theme is actually *lighter*,
which a palette that forgot to invert would otherwise pass while looking
identical to the dark one.

### ✅ The transformation point — and rotate and skew

**Everything could be scaled and nothing could be turned.** The Free Transform
gizmo's eight handles only ever scaled, about the opposite corner; rotation was
two menu commands at 90°; and every anchor in the program was *computed* — the
selection's centre, a corner, an object's bounding box — never chosen. A door
could not hinge on its edge.

**`Object.pivot`** is Animate's transformation point, stored per object:

- In the object's **own** coordinates, before its transform, so it stays where
  it was put on the artwork however the object is then moved, scaled or turned.
  A hinge drawn on a door's edge is still on the edge after the door moves.
- `None` means the centre of what the object actually covers — which needs the
  library for an instance, so `Scene::pivot_of` resolves it. That is exactly
  what every anchor did before, so a document that never touches one behaves
  as it always did, and an object with a point set but still flat renders
  pixel-identically (asserted on the GPU).
- Saved (format version 14), and it **tweens**: a hinge that moves between two
  keyframes moves smoothly rather than jumping at the end.
- Several objects selected together have nothing to keep a point on, so the
  editor holds one for the session — and ignores it the moment the selection is
  a different set, rather than applying one selection's point to another's
  artwork.

**The gizmo learned three gestures**, chosen by where the drag *starts*,
because the pointer's shape there is the promise and changing the answer half
way through a drag would break it:

| Where the drag starts | What it does |
|---|---|
| On the circle | Moves the transformation point — or resets it to the centre, if you press without dragging (Animate's double-click) |
| On a corner handle | Scales about the opposite corner; **Alt** scales about the transformation point |
| Just outside a corner | **Rotates** about it. Shift snaps to 45° |
| On an edge | **Skews** — shears in proportion to the distance from the point, so an edge running through it cannot shear about it |
| Anywhere inside | Moves the selection |

The circle's grab radius is half again a handle's: it is small, it is usually
parked over the artwork you are looking at, and missing it silently *moves the
artwork* instead — the one outcome worth spending a few pixels to avoid.

**3D rotation turns about it too.** `projection_for_object` was already given a
pivot; it was handed the middle of the object's box. It now gets the
transformation point, so a card with its point on the left edge swings on that
edge like a door. Proved on the GPU: hinged, the left edge stays at x=175 while
the rest swings away; about the centre, the same rotation foreshortens the card
inwards and that edge moves to 217.

Flip and Rotate 90° use it as well — unchanged for anyone who never touches the
circle, and hinged where they put it for anyone who does.

**Verified on screen**: the circle drawn at the centre of a selection, dragged
to the top-left corner (artwork unmoved), the rectangle swung 45° about that
corner, the point reset with a click, and the top edge dragged sideways into a
parallelogram.

### ✅ An Assets panel — artwork that outlives the document

The Library holds *this* film's symbols and dies with it. What an animator
accumulates across a series is different: a tree, a lamp-post, a mouth chart,
a walk cycle — made once and dropped into whatever file needs them next.
Without somewhere to keep those, reuse is "open last week's file and copy".

**An asset is a `.buzz` document with one thing in it**, saved under
`%APPDATA%/BuzzAnimate/assets` — beside the workspace layout, by the same rule,
so neither feature invents its own home.

**The folders shown are the folders on disk**, and nothing indexes them:

- assets can be added, renamed, moved and shared by dragging files about in the
  file manager, which is what people do anyway;
- there is no index to drift out of step with what is on disk, and so no repair
  path for when it does;
- an asset is a whole document, so it opens, edits and saves back with no
  second format to maintain — and *placing* one is `Scene::merge`, which
  already renumbers every id it brings across. The importers' path exactly.

**Keeping a selection** goes through the new `Scene::extract(frame, ids)`: a
document holding just those objects, taken **as they are on that frame** (the
same id sits on several keyframes with different transforms), **with the
symbols they depend on, recursively**. An instance whose symbol was left behind
draws nothing, and the asset would look empty on arrival — that is the failure
this is built to avoid, and a symbol that contains an instance of itself does
not send the walk round for ever.

The panel raises intentions — Place, Add, New Folder, Rename, Delete, Rescan —
and the shell performs them, because writing files and merging documents is not
a panel's business and placing an asset has to land in an undo step. A failed
place undoes its own step rather than leaving a "Place Asset" in the history
that changed nothing, and an unreadable library says so: an empty list
otherwise reads as "you have no assets", which is a different and much worse
message.

**Verified on screen, end to end.** A rectangle drawn and selected, `+` in the
Assets panel (which is disabled with "select artwork to add" until something is
selected), then **File ▸ New** for an empty document, then **Place** — and the
rectangle is there in a file that never had it.

### ✅ Named swatches, in folders — the document's palette

**A colour needs a name.** A recent-colours row remembers what you last used,
which is useful for a minute and worthless the next day. A production needs the
opposite: the colours of a show, agreed once and reachable by name. "Hero Skin
Shadow" is a decision; `#C08A6E` is a number that looks like three other
numbers on a swatch strip, and the wrong one gets picked at four in the
morning.

So the palette is part of the **document** — `Swatches` on the `Scene`, saved
(format version 13), undoable, and organised into folders exactly as the symbol
library is:

- `Swatch { id, name, color, folder }`, with folders held as their own set so
  an empty one made in advance survives a save, and deleting a folder moves its
  colours to the root rather than deleting them.
- Ids come from the palette's own counter, not the document's allocator: a
  palette does not interoperate with objects or symbols, and taking from the
  shared allocator would have shifted every other id in a new document by
  however many colours the default palette happens to have.
- Names are made unique on the way in. Two swatches called "Sky" would defeat
  the point of naming them.
- A new document opens with Animate's ten default colours, **named** — Black,
  White, Red, … Orange. A file written before version 13 gets the same palette
  on the way in, which is what such a document effectively had.

**The Swatches panel** lists them as folders and named rows: click sets the
fill, shift-click the stroke, double-click renames, a dropdown per row files it
in a folder, and the footer adds the current fill colour (straight into a
rename, because a colour called "Swatch 7" is a hex value with extra steps).
A Grid view gives Animate's chips for when the names are already known.

**The Color panel now shows the document's palette** above the recent colours,
and names the fill and stroke when they match a swatch — which is the whole
point of naming them. Both rows are kept: the palette is what the production's
colours *are*, the recents are what this session has been using.

**A missing glyph caught, again by looking.** The delete button used `✕`
(U+2715), which egui's bundled fonts do not have — it drew as an empty box in
the screenshot. `theme::font_has` has kept a list of the characters this
project has been caught by since the Actions panel work; `✕` is on it. Now
the trash can, as the Layers panel uses.

### ✅ Edit Multiple Frames, and the onion markers

**Animate's mode, and the one that makes a scene movable.** With **Edit
Multiple** on, every keyframe inside the onion markers is drawn *solid* rather
than ghosted, can be clicked, and is changed together — so a whole scene is
shifted across, scaled or rotated without opening each drawing in turn. It is
the difference between "reference to work against" (onion skinning) and "the
thing I am editing".

- `Scene::update_object_across(first, last, id, f)` changes an object on every
  keyframe *beginning* in the range. The same id legitimately sits on several
  keyframes — F6 clones the `Arc` around a keyframe's objects — so all twelve
  copies of a character drawn on twelve keys move at once. `EditAt` carries the
  range, so the single choke point that already decided Auto Keyframe decides
  this too.
- **Clicking, marquee and Select All all reach the other frames.** A mode that
  shows a drawing it will not let you select would be a worse trick than not
  showing it. The playhead's own frame still wins where artwork overlaps.
- **A selection survives moving the playhead** while the mode is on, because
  the artwork is still on screen. Ordinary pruning drops what the current frame
  does not hold, which would throw most of a scene selection away.

**The onion markers got controls, which they never had.** `Onion::before` and
`after` have existed since Phase 3 with no UI at all, fixed at ±2 — so onion
skinning could never be widened and Edit Multiple Frames would have been useless
tied to it. The transport now shows `markers [2] [2] All` whenever either mode
is on, with **All** covering the whole timeline (Animate's Onion All). Animate
draws the markers as brackets on the ruler and lets you drag them; these are
numbers, which is the deviation recorded in §7.

**Verified on screen.** Three keyframes, each with its own square in a
different place. Edit Multiple on, markers set to All: all three squares appear
solid on the stage at once. Select All then one drag downwards moves all three
— and with the mode switched off again, frame 1 and frame 21 each show their
own square in its new place.

### ✅ Auto Keyframe

**The mode Animate does not have, and every animator working in a span wants.**
Without it, changing artwork at frame 12 of a span changes the keyframe that
*owns* the span — the change reaches back to frame 1, where the drawing began.
That is correct, it is what Animate does, and it is a surprise every single
time. Animate's answer is "press F6 first"; this is the same thing, done for
you, and only when you have asked for it.

With **Auto Key** on (a toggle in the timeline transport and on the Insert
menu), any edit at a frame with no keyframe of its own duplicates the artwork
onto that frame first, so the change starts where the playhead is.

- `Scene::ensure_keyframe(layer, frame)` is exactly F6, as its own operation,
  and `Scene::update_object_where(at, id, f)` is the one place that decides
  whether to call it. `EditAt { frame, auto_key }` carries the two together, so
  the mode is not a bare `bool` threaded through fifteen editing paths where it
  would eventually be passed in the wrong order.
- It covers **everything that changes artwork**: dragging, Free Transform, the
  flips and rotations, Paint Bucket, Ink Bottle, Erase, Smooth, Straighten,
  Convert Lines to Fills, Expand Fill, moving an anchor — and *drawing*, since
  a stroke made on frame 12 belongs to frame 12 rather than to the keyframe on
  frame 1 where it would otherwise appear from.
- **One keyframe, one undo step.** The keyframe is made inside the same edit as
  the change, so moving three objects together makes one keyframe and one undo
  entry, and undoing takes the keyframe back with the change that caused it. A
  mode that leaves keyframes behind for you to delete would be worse than not
  having it.
- Off by default. A mode that silently adds keyframes must be one the user
  asked for, and with it off every editing path behaves exactly as it did.

**A real bug fixed on the way.** The Properties panel edited an object *wherever
its id was first found*, which is the earliest keyframe holding it — so
changing a 3D angle or swapping a symbol while looking at frame 12 quietly
changed frame 1's copy instead. `update_object_at`'s own documentation had
warned about this since Phase 3; the panel was the one caller that had never
been told which frame was showing. It now takes an `EditAt` like everything
else.

**Verified on screen.** A rectangle drawn on frame 1 of a 20-frame span, Auto
Key switched on, playhead to frame 10, rectangle dragged right: a keyframe
appears on frame 10 in the timeline, frame 10 shows the rectangle in its new
place, and frame 1 still shows it where it was drawn.

### ✅ Transform, Swap Symbol, and a looping section

Three of Animate's everyday commands, and one deliberate improvement on it.

**Modify ▸ Transform ▸ Flip Horizontal / Flip Vertical / Rotate 90°** — with
Animate's shortcuts (Ctrl+Shift+9 and Ctrl+Shift+7 for the rotations). Applied
**about the selection's centre, not each object's**: flipping a pair of ears
swaps them as well as mirroring each one, which is what Animate does and what
anybody flipping a pair means. Object by object would mirror each in place and
leave them the wrong way round. Both delegate to the `transform_selection` the
Free Transform work already had, so there is one code path that moves a
selection.

**Swap Symbol** — a `Swap…` menu on the Symbol row of an instance's properties,
listing every other symbol in the library. It points the instance at a
different symbol and **keeps everything else**: where it is, its transform, its
colour effect, its looping, its first frame (pulled back when the new symbol is
shorter). Replacing the instance would lose all of that, which is the entire
reason the command exists.

**A looping section — and it is in the finished film.** Animate's loop is a
preview: the playhead cycles between two markers while you work and the
published file knows nothing about it. That is right for checking a walk cycle
and useless for the thing animators want next — a background that loops eight
times behind a scene, a two-frame flag that flutters for a whole shot — without
duplicating frames by hand.

So this loop is part of the **document**. `LoopRegion { enabled, start, end,
repeats }` on the `Scene`, saved (format version 12), and:

- Playback cycles inside it, including while sound is driving the transport —
  the sound is told to go back with the picture, or the dialogue would run on
  under a repeating image.
- `Scene::playlist()` gives the document frame to draw for each frame of the
  **film**, and `rendered_frame_count()` how long the film is. Without a region
  the playlist is every frame once, so every caller behaves exactly as it did.
- The exporter walks that playlist, so an exported sequence really contains the
  section as many times as asked. The Export dialog's default range is the
  length of the film, not of the timeline.
- The timeline shows the range as an amber band with a tick at each end, dimmed
  when the count is 1 so "on, but doing nothing" is visibly not the same thing.
  The readout says `film 45` beside the numbers, in frames.

**Proved on the GPU, not asserted in arithmetic.** `headless_looping.rs` builds
four frames in four colours, sets frames 2–3 to repeat three times, exports the
sequence, and reads the colour back out of each written PNG: `0 1 2 1 2 1 2 3`.
A second test asserts a document with no region exports exactly what it always
did. Screenshots confirm the band, the readout, and the playhead wrapping back
into the section during playback (frame 18, then frame 9).

### ✅ 3D rotation on an object — §7 item 28, closed

An object can now be **turned in space**: rotated about its own three axes and
pushed forward or back along its own Z. Animate's 3D Rotation and 3D
Translation, and the last thing the spatial camera unblocked.

**Why it matters more than it sounds.** With layer depth and a tilting camera,
a camera move slides layers past each other — which reads as *cards sliding*,
because that is what it is. Giving an object its own angles makes it a plane of
its own, so the same camera move turns it. Three cards at different angles make
a tree that a camera discovers rather than passes; four make a house with a
corner; a body card and two arm cards make a figure that is not a sticker.

- [x] `Spatial` on every object: `rotation_x`, `rotation_y`, `rotation_z` and
      `z`, tweened, saved as format version 11
- [x] Rendered through the object's own plane — and so is **everything inside
      it**, so turning a group turns the group rather than each piece about its
      own middle
- [x] Hit-testing carries the click back onto that plane, so a turned object is
      clickable where it is drawn
- [x] Chrome follows: the selection outline of a turned object is its
      **quadrilateral**, not a box round it
- [x] A flat object renders **pixel-identically** to one from before the
      feature existed — asserted on the GPU
- [x] Exactly edge-on is refused; **past** edge-on shows the card's back,
      mirrored, because that is what the back of a card looks like

**Deliberately on every object.** Animate allows 3D on movie clip instances
only, because its 3D belongs to a display object with a cached surface. Here it
is a plane in a projection and costs nothing extra, so a shape, a group, a
symbol or a rigged character may all have it (§7 item 63).

**A bug the tests caught, and the shape of it.** The object's plane coordinates
are measured from its **pivot**, and `to_origin` already says where that pivot
is relative to the lens. The first version subtracted the pivot's offset *to
the camera* as well, so the artwork ended up measured from the stage's origin
and was drawn distorted and about a twentieth of its size. It looked plausible
in a still — a smaller card is a card — and it was the GPU test that pushes an
object towards the camera and expects it to **grow** that failed.

**Verified by pixels**, in `buzz-export`'s `headless_object_3d` tests: a flat
object is byte-identical, turning a card foreshortens one side, reversing the
rotation reverses which side, tipping it foreshortens top and bottom instead,
pushing it back draws it smaller and pulling it forward draws it bigger — and a
tree of three turned cards *changes shape* when the camera moves, which a flat
drawing cannot do.

**Verified on screen**: a house whose two walls are turned to make a corner,
with the selection outline sitting on the turned wall rather than round it.

---

### ✅ A spatial camera — pitch, yaw, and a real projection

The camera can now **tilt**. Not pan, zoom and roll over a flat picture — tilt,
so a layer's far edge really is further away than its near edge and a rectangle
is drawn as a *trapezoid*. §7 item 28 named the missing piece: a projection
rather than an affine. This is that piece.

**Why an affine could never do it.** An affine preserves parallelism by
definition, and perspective is exactly the failure of parallelism: parallel
lines converge. What does it is a **homography** — a 3×3 matrix on homogeneous
coordinates with a divide at the end. And the reason one 3×3 is enough, rather
than the 4×4 and depth buffer a 3D engine needs, is that every layer here is
**flat**: the perspective image of a plane is precisely a homography of that
plane. No depth buffer, no z-fighting, no sorting beyond the layer order the
timeline already gives.

- [x] `buzz_geom::Projection` — compose with affines both sides, map points and
      paths, invert, and report whether the divide does anything
- [x] **With `g = h = 0` it *is* an affine**, and the render path takes exactly
      the route it always did. That property is load-bearing: it is what makes
      this safe to put under every document ever made
- [x] A projective map turns a cubic into a *rational* cubic, which kurbo
      cannot store, so a tilted path is flattened and mapped — straight edges
      stay exactly straight, which is what makes a trapezoid a trapezoid
- [x] The parts of a path **behind the camera are clipped at the horizon**
      rather than folded back into the frame inside out
- [x] `CameraKey` gains `pitch` and `yaw`, interpolated the short way round and
      bounded well short of edge-on. Format version 10
- [x] Hit-testing and the marquee go through the **inverse** projection, so
      tilted artwork is clickable where it is drawn

**The camera orbits its target; it does not swivel in place.** These are two
different cameras and the difference is the whole feel of the tool. A camera
that swivels keeps its position and turns, so the thing it was looking at
slides out of frame — tilt, and the shot leaves. A camera that orbits stays
pointed at its target, so tilting tips the plane away while what matters stays
in the middle of the frame. The first version swivelled, every GPU test came
back with an empty white frame, and building the camera *behind* its target was
the whole fix. It made the arithmetic simpler too.

**One transform, applied once.** The draw walk used to carry a pre-multiplied
"world" transform alongside the document-space one, and that is what let
lighting geometry be drawn through an object's placement *twice* (§4, the
lighting section). It cannot happen now: geometry accumulates in document space
on the way down and is projected once, at the leaf, by the layer's own
projection. Masks, filters, cast shadows and shading crescents all go the same
way.

**Chrome follows the shot.** Selection handles, bones, warp handles and light
gizmos mark *where something is*, so they are drawn through the document camera
too — and under a tilted camera the selection is a **quadrilateral**, not a
rectangle. This was already subtly wrong whenever the camera panned; it took
tilt to make it obvious. The grid, guides, rulers and stage border describe the
*stage*, which the camera does not move, so they stay where they are.

**Verified by pixels**, in `buzz-export`'s `headless_camera_3d` tests: an
untilted camera is byte-identical, pitch makes one edge measurably wider than
the other, reversing the tilt reverses which edge, yaw converges across the
frame instead of down it, depth still shrinks a far layer *and* still puts it
in perspective, and a camera clamped at the limit still draws a picture.

**Verified on screen**: pitch 55° and yaw 38° together, with the stage in
genuine two-axis perspective — rectangles as quadrilaterals, circles as tilted
ellipses — and the artwork still selectable by clicking where it is drawn.

---

### ✅ The camera has a row in the timeline

Animate shows the camera as a layer above every other, and that is where an
animator looks for it. Enabling the camera now adds a **Camera** row at the top
of the timeline, with the camera's own keyframes drawn on it in the same
conventions as artwork — a key where the shot is set, a tinted run between two
keys where it is being interpolated — so a camera move is read off the timeline
the same way a character's is. Clicking the row selects the camera and picks up
the Camera tool, which is the two halves of the same idea.

**It is not a `Layer`.** It holds no objects and cannot be drawn on. Smuggling
it into the layer stack would mean every piece of code that walks layers had to
know to skip it — and there are a lot of them, in the renderer, the exporter,
the importers and the format. Animate shows it as a layer; underneath, it stays
the camera. Recorded as a deviation in §7 for anyone who goes looking for it in
`LayerStack`.

---

### ✅ A workspace the user arranges — docking, floating, locking

Every panel can now be moved to another side of the window, floated over the
stage, closed, or reordered within its side; the whole layout can be **locked**
so it stops being knocked about; and it is **saved between runs** (§7 item 14,
open since Phase 2, is now closed).

**Not a docking library.** egui has none, and the crate that adds one targets an
egui this project cannot move to — the GPU stack is pinned to wgpu 29, and
egui 0.36 moved to 30. It would also be more machinery than the problem needs:
what an animator wants is a panel on the other side, or floating, or gone, and
the arrangement still there tomorrow. That is a list of panels with a side each.

- [x] `Workspace` — a `Slot` per panel: which side, what order, where it floats,
      and its **home**, the side it returns to when reopened
- [x] Left, Right, Far Right, Bottom, Float, Hidden. The stage is deliberately
      *not* a panel: the thing you are drawing on is not furniture
- [x] Each panel has a `...` menu — the six destinations, plus Move Up and Move
      Down — drawn by the dock rather than by the panel, so **not one of the
      eleven panels needed changing** to become dockable
- [x] A **Window** menu listing every panel with a tick, plus Lock Layout
      (Ctrl+Alt+L) and Reset Layout
- [x] Locking refuses moves, reordering, dragging and resizing — but still lets
      panels be opened and closed, because a locked *layout* is about where
      things are, not whether they are there
- [x] Saved to `%APPDATA%\BuzzAnimate\workspace.json` (`$XDG_CONFIG_HOME`
      elsewhere), never beside the document: a workspace belongs to the person,
      not to the film, and a `.buzz` file handed to somebody else must not
      rearrange their window
- [x] A layout that is missing, unreadable or corrupt gives the default one. A
      window that would not open because its layout file was damaged would be an
      absurd way to lose a day
- [x] A layout saved by an older build **gains the panels it never knew**, so a
      new panel cannot be invisible behind an old workspace

**A check that found four real defects.** Adding the panel menu needed a
hamburger, and this project has shipped a missing glyph twice already. So
`theme::font_has` now measures a character against one nothing on earth bundles
and reports anything the same width as missing — a fingerprint, not a lookup,
and honest about that. It immediately found that **four symbols already in the
interface do not render**: the hamburger itself, `✕` on the new delete buttons,
`▢` on the Layers panel's outline toggle, and `↳` in the Armature panel. The
first two were about to ship; the last two have been empty boxes for phases.
Every symbol the interface draws is now in a list the test walks, and the ones
that are missing are in a second list so nobody reaches for them again.

**Verified on screen**: the Window menu with its ticks, a panel's own dock menu,
the Library floated over the stage, and the whole arrangement still in place —
with "Layout locked" in the status bar — after quitting and reopening.

---

### ✅ Filters and blend modes

Animate's Filters panel — **Blur, Drop Shadow, Glow, Bevel and Adjust Color** —
and its Blend list.

**The hard question, again: what is a filter on vector artwork?** In Animate a
filter is a raster effect: the movie clip is rendered to a surface and the
surface is blurred. That is the obvious implementation and the wrong one here,
for exactly the reason lighting is not shaded per pixel — Vello offers no
shader hook, and a raster post-pass throws away the property this whole project
is built on. A blur baked at 100% is a smear at 10 000%.

So a filter is **paths**:

- **A soft edge** is a fill plus a ramp of concentric strokes, each wider and
  more transparent than the last. A point *t* outside the edge is covered by
  every stroke wider than *2t*, so coverage falls off smoothly with distance: a
  blur of the silhouette with **no booleans, no offsetting and no buffers**
- The alphas are chosen so the *cumulative* coverage follows the profile —
  `alpha[i] = (target[i] - target[i-1]) / (1 - target[i-1])`. Get this wrong
  and the result is either flat or banded; it is the whole difference between a
  soft edge and a set of rings
- **A drop shadow** is that soft edge, in the shadow's colour, offset by angle
  and distance, drawn behind. **A glow** is the same thing centred. **A bevel**
  is a highlight and a shadow along the edge, clipped to the shape — which is
  what an edge lit from one side looks like
- **An inner** shadow or glow is the region between the shape and an offset
  copy of it, drawn with the **even-odd** rule so no boolean is needed there
  either
- **Adjust Color** — brightness, contrast, saturation, hue — is exact
  arithmetic on the colours, applied through the whole subtree

**Blur is the one that costs.** It has to fade on the *inside* of the edge as
well, and no amount of stacking translucent copies can take coverage away. So a
blur is real offset geometry, shrunk through to grown, one boolean per band —
and therefore cached, on copy-on-write pointer identity, exactly as lighting
geometry is.

**An elliptical blur is elliptical.** Blur X and Blur Y are separate, as they
are in Animate, and the anisotropy is carried by the *pen*: the path is
squashed so the blur is round, stroked with a round pen, and path and pen are
stretched back together. The first version scaled the stroke width instead,
which quietly gave a round pen again and lost Blur Y entirely.

**Blend modes** map onto Vello's own: Darken, Multiply, Lighten, Screen,
Overlay, Hard Light and Difference are mixing modes, Add is `Plus`
compositing, and Layer is the isolation group itself. Every mode but Normal
draws into a group, because a blend equation needs a backdrop to blend *with*
and without a group that backdrop is the whole stage.

**In the document** (format version 9)
- [x] Filters on any object, and on a **layer** — Animate has neither; there a
      filter belongs to a movie clip instance, and blurring a background means
      selecting it all and making it a symbol first (§7 items 52 and 53)
- [x] Filters **tween**, matched by position and by kind, with the shadow angle
      taking the short way round. A glow that grows and a shadow that swings
      are most of what filters are used for
- [x] A filter kind this build does not know is **refused** rather than
      dropped: silently losing an effect is worse than saying the file is wrong
- [x] A document with no filters renders **pixel-identically** to one from
      before the feature existed — asserted on the GPU

**Verified by pixels**, in `buzz-export`'s `headless_filters` tests: an
unfiltered document is byte-identical, a drop shadow lands on the side the
angle says and swings when it changes, a glow surrounds the artwork and fades
outwards, a blur softens a hard edge into a ramp, Adjust Color lifts every
channel, knockout keeps the shadow and drops the artwork, Multiply darkens an
overlap, a layer filter reaches every object on the layer, and a disabled
filter paints nothing.

---

### ✅ Layer parenting, and a symbol for every tool

**Layer parenting** is Animate's other rig: link a layer to another, move the
parent's artwork, and the child's artwork goes with it. A head follows a body,
an arm follows a shoulder, and none of it needs a bone.

- [x] `Layer::follows` — deliberately *not* `Layer::parent`, which is the folder
      a layer is filed in. Animate keeps the two apart because a layer can be
      in one folder and follow a layer in another, and so does this
- [x] **Motion is inherited, not position.** What passes down the chain is how
      far the followed layer has travelled *from its own first keyframe*.
      Inheriting its absolute transform would fling the child across the stage
      the instant the link was made, which is not what parenting means
- [x] Chains compose outermost first, so a hat follows a head follows a body
- [x] Cycles are refused when the link is made (`can_follow`) **and** survived
      when one is read from a corrupt file — the walk is bounded by the layer
      count
- [x] Deleting a layer releases whatever followed it, rather than leaving a
      Parent column pointing at a layer the user cannot find
- [x] Inherited motion reaches the spatial index and the hit-test as well as
      the renderer: a parented limb that is visible but unclickable would be
      worse than one that does not move at all
- [x] Symbols have their own follow links, so a character symbol can be rigged
      inside itself
- [x] Saved as format version 9; a document without the field loads with every
      layer following nothing

**Which transform is "the layer's"?** A layer has none — its objects do. This
takes the **first object on the layer** as the thing that represents it,
because that is how layer parenting is used: one symbol per layer, which is
Animate's own documented rigging workflow. Recorded as a deviation (§7 item 50)
rather than hidden.

**Verified by pixels**, in `buzz-export`'s `headless_parenting` tests: an
unparented document is byte-identical, a head that is never keyed moves when
its body does, it moves *half way* half way through a tween, and a hat inherits
both its head's motion and its body's.

**Bug found while writing those tests** — and it was a real one, in the test
rather than the feature: F6 past the end of a span makes a *blank* keyframe
(§7 item 37), so the body had no artwork on frame 10 and therefore no motion.
The same defect an animator meets, met from the other side.

---

**A symbol for every tool.** The tool strip showed shortcut letters — `V`, `A`,
`Q`, `P` — which is what the palette of a program that has not drawn its icons
yet looks like. All twenty-two tools now have one, following Animate's shapes
so the Ink Bottle is where a user of it would expect.

**They are drawn, not typed.** Every one of these has a Unicode codepoint, and
none of them render: egui's bundled fonts carry a small subset, and a missing
glyph comes out as an empty box. That has bitten this project twice already
(`▼` in the Library, and again in the Actions panel) and it is invisible to
every test — only a screenshot shows it. So each symbol is built from lines and
polygons in a unit square: it cannot fail to render, it is sharp at any button
size, and it costs a handful of vertices.

`tool_shapes` returns the shapes rather than painting them, which is what makes
them testable at all: every tool draws something, no two draw the same thing,
nothing escapes its button, and a wide button gets a centred square symbol
rather than a stretched one.

---

### ✅ Lighting — a sun, a sky and a lamp

Not an Animate feature at all: Animate has no lights, and this was asked for by
name, as Blender does it — *put a sun in and the colours, the highlights and
the direction of every shadow follow it*. Recorded as a deviation (§7 item 44)
with the reason, like every other departure.

**The hard question first: what does lighting a *vector* drawing even mean?**
Vello has no shader hook, so there is no per-pixel lighting to be had, and a
raster post-pass would throw away the one property the whole project is built
on — that artwork survives 10¹⁴% zoom. So light is expressed as **geometry**:

- **The shaded side** is a crescent cut from the shape's own outline, offset
  away from the light — a boolean difference of the path against itself moved
- **The highlight** is the same construction towards the light, smaller
- **The cast shadow** is the silhouette projected onto what is behind it: a
  *translation* for a sun (parallel rays) and a *scale about the lamp* for a
  point light, which is what makes a lamp's shadows splay outwards
- **The fill itself** is tinted by the light's colour, mixed in linear light so
  a warm key warms the artwork rather than washing it grey

Everything stays editable, zoomable, exportable and undoable, because it is all
paths in document space.

**`buzz-light`** — the model and the maths, knowing nothing about documents
- [x] **Sun**: azimuth and elevation, parallel everywhere. One direction, one
      shadow direction, shadow length set by how low it stands
- [x] **Sky**: two colours, overhead and horizon, mixed by how high on the
      stage a shape sits. Ambient — it fills, and casts nothing
- [x] **Lamp**: a point on the stage at a height, with falloff. `radius` is
      "the distance at which it is half as bright", a number an animator can
      reason about, rather than an inverse-square constant
- [x] `standing_height` — how far flat artwork is assumed to stand off the
      background. Flat drawings have no thickness, so without this nothing
      would cast anything; layer depth (Phase 3) adds to it, so a layer pushed
      forward really does throw a longer shadow
- [x] Shadow length is **bounded**: a sun on the horizon would cast an infinite
      one, so it is clamped at twelve times the standing height

**One key light, on purpose.** Shading and cast shadows follow the strongest
directional light; the rest contribute colour and fill. Summing crescents from
every light is physically nicer and visually a mess — two overlapping
terminators on flat artwork read as dirt, which is why hand-drawn animation
lights from one key and fills with the rest.

**In the document** (format version 8)
- [x] A rig on the `Scene`: the lights, an on/off switch, a fill colour and a
      modelling strength, all undoable and all saved
- [x] **A document with no lights renders pixel-identically to one from before
      the feature existed** — asserted on the GPU, not assumed

**Rendered, with a cache.** Booleans are not free, so shading geometry is kept
between frames in a `LightCache` keyed on the copy-on-write `Arc`'s *pointer
identity*: editing one shape rebuilds that shape and nothing else, because
structural sharing means every untouched object is literally the same
allocation. Entries unused for three frames are dropped.

**The panel and the on-stage gizmos**
- [x] Insert ▸ Light ▸ Sun / Sky / Lamp, and the same three buttons in the
      Lighting panel — one path, `Editor::add_light`, so both behave alike
- [x] Colour, strength, direction, height, reach, shadows and their strength,
      standing height and softness; plus the rig's fill colour and modelling
- [x] **A dial, not two sliders.** The sun's direction and height are one
      gesture in the world — you point at the sun — so the panel and the stage
      both give it a dial: which way round the handle sits is the azimuth, how
      far out it sits is the elevation, rim being the horizon and middle
      straight overhead. That is the mapping a fisheye photograph of the sky
      uses, and it makes "put the sun over there" one drag
- [x] The **dark spoke opposite the handle is where the shadow will fall**, so
      the gizmo shows the consequence rather than the angle
- [x] A lamp is drawn where it is, with a reach ring that is also a handle;
      a sky gets no gizmo, because a handle that did nothing is worse than none
- [x] One drag is one undo step, and drags are unsnapped — a handle is where it
      is drawn, and a click that jumped to the nearest artwork edge would miss
- [x] Handles are chrome, drawn by the stage overlay, so they can no more reach
      an exported frame than a selection rectangle can
- [x] Ctrl+Shift+L hides and shows them; hidden handles cannot be grabbed
      either, so what can be seen is exactly what can be taken hold of

**Verified by pixels**, in `buzz-export`'s headless lighting tests: an unlit
document is byte-identical, a sun changes the picture without blackening it,
the shaded side follows the sun, the cast shadow falls away from it and swings
round when it does, a lower sun throws a longer one, a warm light reads warmer
than a cold one, a lamp lights what is near it more than what is far, and a sky
fills without casting.

**Verified on screen too**, which is where three defects were found that no
test could see:

1. **The crescents were invisible.** Two causes at once: the geometry is built
   in *document* space and was being drawn through the accumulated world
   transform, applying each object's placement twice; and the shadow pass and
   the shading pass shared a cache key while asking for different arguments, so
   the shading pass was handed geometry built with modelling switched off. Both
   passes now ask for exactly the same thing, so one entry serves both and the
   booleans are paid for once.
2. **A lamp lit two squares either side of it equally**, because illumination
   was worked out once per layer. A lamp's whole character is that it varies
   across the stage; it is now per shape.
3. **The gizmo was invisible on a white stage.** A warm key light is very
   nearly white, and the handle was drawn in the light's own colour. Structure
   is now drawn in ink — dial, spoke, rays and outline — with the light's
   colour only as the handle's fill, which is what Blender does too.

### ✅ The Animate importer, rewritten against real films

The first pass at this read the format's *documentation*. Pointed at four of
the studio's own films — 45 to 140 MB, thousands of symbols each — it produced
streaks. Six defects, each found by rendering a frame and looking at it beside
the film's own still.

**0. A hex coordinate is signed, and its width carries the sign.** This one
did more damage than the rest together. XFL writes coordinates in hex twips,
and a negative number is written at full width in two's complement:
`#FFFFFA.21` is **minus six twips**. Read as unsigned it is eight hundred
thousand pixels. One point like that in a shin stretched the leg to four
hundred thousand units across; every shape holding one threw a straight spike
across the frame, which is what the "streaks" over every imported film were.
Short forms stay positive — `#82` is 130 twips, not −126 — because Animate
only writes the leading `F`s when it means a negative number. On the studio's
own film this fixed the elongated limbs and every stray line in the same
change.

**1. A `DOMShape` is not a set of outlines.** It is a soup of boundary pieces,
each saying which fill lies on its *left* and which on its *right*, written in
drawing order and split across several `<Edge>` elements; a piece is usually
two points long. Read as one closed outline per edge — the obvious reading — a
bush arrives as three hundred slivers. The pieces are now **reassembled**: for
each fill, every piece with it on the left, plus every piece with it on the
right turned round, chained end-to-start into closed loops. On one real
document that is 32 180 fragments becoming 2 033 shapes, and the difference
between green streaks and a village at night.

**1a. Where several boundaries meet, the loop turns the corner.** Grass blades
from a common root, two shapes sharing an edge: at that point three or four
pieces start, and taking whichever was stored first walks the loop into the
neighbouring shape and back. The piece taken is now the one that turns
furthest back towards where the loop came from — the standard rule for tracing
a face of a planar subdivision. Stated plainly: it did **not** visibly change
any of the four films tested, and it is kept because "first stored" is not a
rule at all.

**2. The layer order was upside down.** The file's order *is* the timeline's,
top first; this read it as bottom-first and reversed every stack. Skies drew
over artwork, backgrounds over the characters standing on them — and every
mask sat *below* the layers it should clip, so masking did nothing at all
anywhere. The camera layer proves the order: Animate pins it to the top of the
timeline and writes it first.

**3. Masked layers are not marked `layerType="masked"`.** Whatever the format
allows, Animate writes an ordinary layer whose `parentLayerIndex` points at the
mask above it — the same attribute a layer in a folder uses for its folder.
Waiting for a value Animate never writes meant every mask in every real
document claimed nothing.

**4. A rigged character's parts are stored *relative* to what they hang off.**
Animate's Layer Parenting is a third relationship in a fourth place:
`layerRiggingIndex` on the parent, `parentLayerIndex` on the child's **frame**
(it can be re-parented mid-shot). A head is stored as a small offset from its
torso — `(49, -156)` — not as a position. Our model, following Animate's
*editing* behaviour, takes a child's transform as absolute and propagates only
the parent's motion away from rest. Before this every character imported in
pieces, heads a few hundred units up and left of the shoulders they belong on.

The chain is composed **per keyframe** on the way in — `child_world(f) =
parent_world(f) * child_relative(f)`, parents baked before children — and the
link is then dropped. Baking the parent's *rest* pose instead is exact for one
level and wrong for two, because matrices do not commute: by the shin the two
products disagree and the leg comes out bent the wrong way.

**5. Only an instance's own matrix places it.** `<Matrix>` appears in several
places in XFL and most of them are not placements: a gradient carries one to
say where its ramp runs, a bitmap fill to say how its image lies. Applied to
"the last object drawn", a gradient's matrix — whose scale is a fraction of a
percent — collapsed whichever instance preceded it to a point. A lantern, a
bed and a cot vanished from a shot while the layer still reported them there.

**6. The camera holds between untweened keys.** Ours interpolated between
every pair, which is right for a tweened move and wrong for the nineteen held
spans in one real film: a shot that should sit still for ten seconds and cut
was drifting the whole way. A held span is now written as a second key with the
same values at its last frame, so the hold lives in the track rather than in a
rule to remember.

**And two smaller ones.** A graphic instance's `loop` and `firstFrame` were
ignored, so 535 held poses in one film played through their whole timelines and
485 first-frame offsets were lost; and instances played against the *timeline's*
frame number rather than the frames since they were placed, putting every cycle
at an arbitrary point in itself.

**How any of this can be checked.** Two examples, because import fidelity
cannot be settled by counting shapes — a drawing that arrives as the right
number of wrong outlines passes every count there is:

```text
cargo run -p buzz-import-xfl --example report -- "scene.fla" --layers 600
cargo run -p buzz-export     --example shot   -- "scene.fla" 600 out.png
```

The first lists what came across and what each layer holds at a frame; the
second renders that frame to a PNG, headlessly, so it can be put beside what
Animate shows.

### ✅ A real Animate document imports — nested symbols, the camera, and damaged files

An animator opened one of their own films and got a page of *instance of unknown
symbol*, several hundred lines of it. Three separate defects, each found by
pointing the importer at the real thing rather than at a fixture.

**Symbols could not see each other.** Every symbol was parsed against the
symbols parsed *before* it, so a torso holding an arm found the arm only if the
archive happened to store the arm first. In a rigged character almost nothing
resolves that way: the parts are defined after the thing that holds them.
Names and ids are now collected from every library file first, and the
timelines read second, so ordering cannot matter. On one real document that is
**2 908 instances that used to be dropped**, and the lookup prefers the full
library path over the bare name so two folders may each hold a `head`.

**Animate's camera was an unknown symbol.** The camera layer holds instances of
`__Camera__`, which is not in the library — it is Animate's own. Rather than
special-case it into silence, the camera is now *imported*: the layer's
keyframes become camera keys, and since the matrix places the camera rather
than the view, the zoom is its inverse (a camera scaled to a half shows half
the stage, which is a zoom of two). The camera layer does not become a layer of
artwork, because in Animate it never was one.

**Animate writes broken files.** Three symbols in one document contain
`<DOMShape` with no closing bracket, immediately followed by `</DOMShape>` —
puppet-warp shapes, saved damaged by Animate itself. Refusing the file threw
away every other frame in the symbol with the bad one, and the symbol then
vanished from every scene using it. What reads is now kept and the truncation
is named in the report.

**Measured on four real documents** of 45–140 MB: from hundreds of unknown
symbols to none. What is still reported is honest — gradients flattened,
bitmaps skipped — and `cargo run -p buzz-import-xfl --example report -- <file>`
is how any of it can be checked without opening the window.

### ✅ Edit in place: the scene stays, paled

Inside a symbol the stage used to show the symbol alone against empty grey —
which says nothing about whether the drawing is right. A head has to be judged
against the shoulders it sits on. The document's own timeline is now drawn
first and then veiled in translucent white, with the symbol's contents solid
on top: everything sharp on screen is the thing being edited, and everything
pale is where it belongs.

The veil covers the whole visible area rather than the stage rectangle,
because the scene runs off the edges of the stage as freely as the artwork
does and a veil that stopped at the stage would leave a bright ring of
unveiled drawing around it. The context is drawn unlit — shading and cast
shadows on reference artwork read as dirt on the stage.

### ✅ Double-click goes into a symbol, and out again

Animate's whole navigation, and it was missing: the only way into a symbol was
the Library, one at a time, with no way to tell which of three like-named heads
was the one on screen. Double-clicking an instance now edits it where it
stands, and double-clicking past the artwork comes back out a level. Ctrl+E
changed with it: the **selection** leads and the Library is the fallback, since
the Library keeps its highlight for as long as the panel is open and was
winning every time.

### ✅ Ctrl+wheel zooms; the wheel alone scrolls

The wheel zoomed by itself, which makes a long document impossible to walk
down — and it zoomed with **Alt** held too, which is where the report came
from. The cause is worth writing down: egui's `vertical_scroll_modifier` is
Alt, so holding Alt forces the wheel onto the vertical axis, and the old code
zoomed on anything that arrived on that axis. Any modifier at all still
zoomed.

Now the wheel pans and **Ctrl+wheel zooms about the pointer**, which is
Animate's arrangement and every drawing program's. egui has already turned
Ctrl+wheel — and a trackpad pinch — into a zoom factor and taken it out of the
scroll delta, so the two cannot fight over the same event; Shift for
horizontal comes free from the same place.

Verified by driving the wheel at the window: plain 55.56% → 55.56%, Alt
55.56% → 55.56%, Ctrl 55.56% → 101.23%.

### ✅ A ruled timeline, not a boxed one

Every frame cell was drawn as a full rectangle, so each grid line was drawn
**twice** — once as one cell's right edge and again as its neighbour's left —
and every row carried a line along its top and its bottom. At twelve pixels a
cell that doubled ink was most of what the timeline showed: rows looked
separated by a gap they did not have, and the frames were hard to count. One
hairline down the right of each cell and one along the bottom is Animate's
grid, and it is half the ink for the same information.

The two sizes now show their numbers as well — `12 px`, `100%` — because a
bare slider says a size can be changed and not what it is, which is the one
thing somebody matching two documents needs.

### ✅ The transformation point moves under the pointer

Dragging Animate's white circle worked and looked as though it did not: the
point is moved by **one** edit when the drag ends — not one per pixel, which
would fill the undo history with a hundred steps — so the circle sat still
under a moving pointer until the button came up. It is now drawn where the
pointer has it for the length of the drag, which is the whole difference
between a control that works and one a user gives up on.

Three tests pin the behaviour itself: on loose artwork, on a symbol instance
— the case that matters, since a character turns about a hip or a shoulder and
both are on an instance — and on the **Selection** tool, which can now move it
too. That last is a deviation from Animate, and a deliberate one: the point is
what a rotation, a skew and an Alt-scale all turn about, and having to change
tools to move it and change back to carry on selecting is a step nobody thanks
you for. Only a *drag* counts, so clicking the middle of a shape still selects
and moves it exactly as before, and the circle is drawn for the selection tools
so that what can be grabbed can be seen.

### ✅ The timeline: layer buttons, a centred transport, and two zooms

**New Layer, New Folder and Delete under the layer names**, at the bottom left
where Animate keeps them and where the hand goes looking. They raise the same
three commands as the Insert menu and the Layers panel — a third door onto one
room, not a third implementation — so undo covers them like anything else.

**The transport is centred.** egui lays widgets out as it draws them, so the
row's width is not known until it has been drawn; the previous frame's width
decides the leading space. The contents change about once a session, so the
one-frame correction is not visible. The frame buttons moved into the row
rather than staying pinned right, because a right-aligned group takes all the
remaining width and leaves nothing to centre within.

**The rows touch.** egui's standard item spacing was putting a stripe of panel
background between every layer, which cost a row of layers for every three
shown. The cells' own outlines separate them, as they do in Animate.

**Two sliders, because the useful size depends on the film.** A
four-thousand-frame timeline wants narrow cells; a twelve-frame cycle wants
wide ones. Frame width and row height are workspace state — saved with the
layout, not with the film, and deliberately *not* undoable, since Ctrl+Z after
a zoom should undo the last edit and not the zoom. The ruler thins its labels
as the cells narrow: every fifth frame, then every tenth, then every twentieth,
because overlapping numbers are worse than fewer.

### ✅ The inverse mask — a mask that hides what it covers

Animate has no such layer. A hole is cut there by drawing the mask as a shape
with a hole *in* it, which means redrawing the mask by hand whenever the
artwork under it moves. `LayerKind::InverseMask` is the same region used the
other way round: the run of masked layers below shows everywhere **except**
where the mask covers them. A character walking behind a foreground element, a
scratch-off, smoke eating a title.

**It is not a reversed clip, and the reason is worth keeping.** The obvious
implementation — a big rectangle with the mask's subpaths reversed inside it —
is wrong for any mask made of overlapping blobs: under the non-zero rule two
reversed overlapping shapes wind back to *filled*, so the overlap would show
the artwork through the middle of its own hole. Instead the masked run is drawn
into a group and the mask is punched out of it with `DestOut`, which is exact
whatever the geometry does. The group is a render target, so it is bounded by
what the masked layers actually draw, with a margin for strokes and filters.

Both cases are proved on the GPU: one test that the hole is a hole, and one
with two overlapping shapes that the *middle* of the hole is a hole too — the
assertion that fails for the reversed-clip implementation.

**And layers can now be given a type at all.** Until this the only way to get a
mask was to import one: `LayerKind` was in the model, in the renderer and in the
file format, with nothing in the interface that set it. The Layers panel now has
a type per layer with a line of explanation on each, and masking stays
positional — set one layer to Mask or Inverse Mask, the ones under it to
Masked, and the stack does the rest.

### ✅ Gradients — §7 item 8, the most-cited gap, closed

Every fill and every stroke in the program was one colour. Now a fill or a
stroke is a **paint**: either a colour or a ramp.

**The model** (`buzz-scene::gradient`)
- [x] `Paint::Solid` or `Paint::Gradient`, on `FillSpec` and `StrokeSpec` alike
- [x] Linear and radial, with stops, a spread mode (Animate's Extend, Reflect
      and Repeat) and a radial focal point
- [x] **A unit gradient plus a matrix, not two points on the stage.** The ramp
      runs from `x = −1` to `x = 1` in its own space and a transform puts it
      where it belongs. That is what SWF and XFL both store, because it is what
      Flash compiled; it is what Animate's Gradient Transform tool edits —
      three grips *are* a matrix; and it makes a squashed radial gradient free
      rather than a second radius and a rotation bolted on.
- [x] `Paint::color()` — one colour standing in for a ramp, weighted by how
      much of the ramp each span occupies, so a gradient that is red for nine
      tenths of its length averages to red rather than to the midpoint. This is
      what the lighting model, outline view and the colour wells read.
- [x] Degenerate input is **repaired, not refused**: no stops, one stop, forty
      stops, coincident stops and a NaN offset all produce something drawable.
      A gradient that renders as nothing is a silently invisible shape.

**Bug found by its own test — `f64::clamp` propagates NaN.** `offset.clamp(0.0,
1.0)` on a NaN leaves the NaN, so a damaged file's stop offset passed straight
into the model, where `partition_point` then returned whatever it liked. The
sort was already safe (`total_cmp`, chosen for exactly this reason); the clamp
beside it was not.

**In the renderer**
- [x] The gradient is handed to Vello as a **brush transform**, composed in
      `f64` from the object's placement, the camera and the render split — the
      same chain the path takes. Vello multiplies it by the same `gpu_view` the
      path is drawn with, so the ramp cannot drift off its artwork at any zoom.
- [x] Colour effects, Adjust Color and the onion-skin ghost reach a gradient
      **stop by stop**, so a tinted instance of a gradient-filled symbol tints
      the whole ramp instead of flattening it
- [x] The lighting crescents take the fill's paint stop by stop too: the shaded
      side of a gradient-filled shape is that gradient darkened

**In the file** — format version 16. A fill writes `color` **or** `gradient`
and never both, so there are not two answers in the file to one question.
Every older file still loads: they carry `color` and no `gradient`, which is
exactly what a solid paint deserialises to, and a document using no gradient is
written byte-identically to what version 15 wrote.

**In the importer.** XFL gradients arrive as gradients. Two things had to be
right: the `ratio` on each `<GradientEntry>`, which was **discarded entirely**
while gradients were averaged — an average does not care where its terms sit,
and a file whose middle stop is at a quarter draws nothing like one where it is
at a half — and Flash's gradient box, a fixed square 1 638.4 pixels across that
the file's matrix maps onto the artwork. A test pins that number, because it is
the one value that decides whether an imported gradient is the right size. The
gradient's own `<Matrix>` is claimed before anything else can mistake it for a
placement, and a test asserts an instance following a gradient still gets its
own matrix — the failure that used to collapse a lantern to a point.

**In the interface**
- [x] The Color panel has Animate's fill type — Solid, Linear, Radial — with a
      live ramp, stops that can be added by clicking the ramp, dragged and
      recoloured, the spread mode and the focal point
- [x] A new stop takes **the colour the ramp already has there**, so adding one
      never changes the picture until it is moved
- [x] The gradient is fitted to the shape being drawn, so drawing a rectangle
      with a gradient selected produces a ramp across that rectangle. The Paint
      Bucket fits it to the shape it is poured into — a ramp laid across
      somebody else's bounds shows one flat colour and reads as the tool having
      done nothing.
- [x] **The Gradient Transform tool works**, which §7 item 8 recorded as inert.
      The grips are the matrix's own parts — the centre is its translation, the
      end of the ramp its first column, the width handle its second — so
      dragging one is a write to two numbers rather than a decomposition into
      an angle and a scale. A skewed gradient therefore stays skewed when it is
      dragged, which a rebuild-from-parts implementation quietly straightens.
- [x] The focus is tested for **before** the centre, because on every gradient
      nobody has adjusted the two coincide: its default is zero. Testing the
      centre first makes the focus unreachable.
- [x] Merge-shape fusion asks whether two paints match, not whether two colours
      do. A red-to-blue ramp and a blue-to-red one share an average, and fusing
      them would throw one away.

**Proved on the GPU.** `headless_gradients.rs` renders through the same path the
window uses and reads the pixels back. The assertions are about *where* each
colour lands, because the path is pre-transformed on the CPU while the brush is
placed by a matrix Vello composes on the GPU — two routes to the same pixel, and
a gradient that slides off its artwork is what a disagreement looks like. Six
tests: a linear ramp runs the right way and never doubles back; a **moved
object carries its gradient with it**; a radial gradient cools in all four
directions; stop offsets put the colour where the file says; Reflect mirrors
where Pad holds; and a solid fill still comes back exactly solid.

### ✅ A five-minute horror short, built and measured — and four defects

Every other section here records a feature. This one records an **exercise**:
building what a rural horror short actually needs, at the scale it actually
reaches, and putting a stopwatch on every stage. A tool can pass every unit
test and still be unusable, because the failures that stop a production are
not wrong answers — they are things that take four minutes when they should
take four seconds, on a document the size a real film gets to.

`buzz-app/tests/horror_short.rs` builds a villager rigged out of reusable
part-symbols, a night exterior on depth-separated layers, a moon and a lantern
casting shadows, fog and glow as filters, a vignette as an inverse mask, an
armature posed with IK, five minutes of dialogue lip-synced — 7 200 frames —
and then *works on it*. It also writes the frame out, because numbers say a
render is fast and only a picture says it is right.

**Four defects came out of it, three of them severe.**

**1. Lighting rebuilt the whole film on every mouse move.** Generated lighting
geometry is cached, and the cache was thrown away whenever the *document's*
revision changed — which bumps on every edit, and on every mouse move of a
drag. Dragging one hand rebuilt the shading crescent and cast shadow of every
shape in the film, once per frame, for as long as the drag lasted: **770 ms a
frame**, measured. `LightRig::fingerprint` keys the cache on the lights
themselves, so an edit to artwork keeps it and only the objects that actually
moved miss. **770 ms → 17 ms.**

**2. A symbol instance cast no shadow at all.** The note in `cast_shadows` said
so and it reads as a small gap; it is not. A document imported from Animate is
*entirely* symbol instances, so a real film cast no shadows whatever —
switching shadows on did visibly nothing, which looks like a broken feature
rather than an unfinished one.

**3. A mask painted itself into the film.** A mask layer's artwork was skipped
only when the mask was actually clipping something, so a mask added before the
layer beneath it had been set to Masked was drawn as ordinary artwork — opaque,
full size, over everything. The natural order of work walks straight into it:
you draw the stencil first and say what it masks second. On the vignette here
it covered the entire frame. A mask that claims nothing is still a mask; the
only case where one should show its own artwork is Animate's editing rule, so
the skip is unconditional for an export and conditional only for the stage.

**4. Onion skinning rebuilt every blur, six times a frame.** One frame on
screen is a dozen draws — the scene behind an opened symbol, every keyframe
under Edit Multiple Frames, the ghosts either side, and the live frame. Each
opened and closed the caches on its own, and both evict anything not drawn
within three generations, so seven passes over one screen frame aged the first
out before the last had finished. **257 ms a frame** with three ghosts either
side: four frames a second, in the mode an animator spends most of their time
in. The generation now belongs to the *screen* frame. **257 ms → 5.0 ms.**

**What it measures, on the 14700K and the 5060 Ti:**

| Stage | Time |
|---|---|
| Build the five-minute document | 1 ms |
| Scrub anywhere in it | under 3 µs |
| Select a layer's artwork | 6 µs |
| Click a villager, go inside it | 40 µs |
| Analyse five minutes of dialogue | 198 ms (7 200 viseme frames) |
| Write 1 799 mouth keyframes | 1 ms |
| Waveform for the whole take | 14 ms |
| First lit frame, everything cold | 52 ms |
| Worst frame during a lit 30-step drag | **1.35 ms** |
| Worst frame while scrubbing the film | 1.12 ms |
| Worst onion-skinned frame, 6 ghosts | 5.0 ms |
| Merge-shape stroke on a busy layer | 1.7 ms |
| Sixty mouth swaps | 157 µs |
| Undo a whole drag | 36 µs |
| Save the film / reopen it | 11 ms / 7 ms |
| Export the whole film, NVENC | 103 s |

**And a heavy scene**, which is what "must not hang on complex scenes" actually
means: a cast of thirty nested four symbols deep, a three-hundred-piece set,
a hundred and fifty fogged objects, all lit. First frame 305 ms, then **6 ms**
a frame, and **12.7 ms** a frame while dragging.

---

### ✅ Bitmaps, the Lasso and the Magic Wand — §7 items 10 and 22 closed

**One decision made all three of these cheap: a bitmap is a shape filled with
an image.**

The obvious model is a bitmap *object kind* — a picture with a position and a
size, beside shapes and instances. Every editor that took that road then had to
add "Break Apart" to escape it, because a placed bitmap can only be moved and
scaled, and everything an animator actually wants to do to a photograph — cut
the sky out, rub away an edge, keep the tree — needs it to be artwork.

So there is no bitmap object. There is `Paint::Image`, beside `Paint::Solid`
and `Paint::Gradient`, and a placed bitmap is an ordinary rectangle filled with
one. It arrives already broken apart, because there is nothing to break: the
booleans, the eraser, the subselection anchors, masking, tweening and Convert
to Symbol all work on it the moment it lands, none of them knowing a picture is
involved. The fill carries a **unit-square transform**, exactly as a gradient
does and for the same reason — so cutting the shape does not slide the picture
about inside what is left. That is `cutting_the_shape_leaves_the_picture_where_it_was`,
and it is the claim the whole design rests on.

**Import.** File ▸ Import Image, decoding PNG, JPEG, GIF, BMP and WebP into
straight-alpha RGBA8. The source bytes are kept alongside the decoded pixels so
a saved document re-writes the file it was given rather than a re-encoding of
it, in `media/image-N.ext` — the container directory reserved since Phase 1.
Format version 17. A document whose bitmap has gone missing opens with grey in
its place rather than refusing to open.

**The Lasso is back, and this time it cuts.** It was taken out of the palette
in the previous iteration for being a greyed-out promise. A lasso round part of
a drawing cannot *select* "the left half" — no such object exists — so it makes
one: the region is cut out of every shape it crosses, and the piece inside is
selected. Delete then removes it. That is Animate's behaviour on a shape, and
it is the only reading of the tool that is worth having. Instances and groups
are picked whole instead, because cutting one would mean cutting the symbol and
every other instance with it.

**The Magic Wand** floods from the clicked pixel, traces the boundary of what
it took, and hands back a **path** — not a pixel mask, which there would be
nowhere to keep and nothing to do with. Three steps, each with a reason:

- **Flood** by scanlines, not by neighbours: a run per stack entry rather than
  a pixel, because the recursive version overflows the stack on any real
  photograph. Tolerance judges alpha separately from colour, so the wand
  spreads across the transparent part of a cut-out — where the RGB under a
  transparent pixel is whatever the encoder happened to leave.
- **Trace** as unit lattice edges chained into loops. Holes come out wound the
  opposite way from outlines, so a non-zero fill leaves them open with no extra
  bookkeeping — `a_ring_traces_with_its_hole_open`.
- **Simplify**, because the honest answer is a hundred thousand segments for a
  sky and every boolean afterwards would pay for it. Collinear runs collapse
  exactly; Douglas–Peucker takes the rest. The epsilon *doubles until the path
  fits a 20 000-point budget* — a guarantee rather than a hope, because a grainy
  photograph at a high tolerance makes every pixel a corner and no fixed epsilon
  tames it.

On vector artwork the wand selects the shape, because a region of one colour
*is* the shape someone drew. A user who does not know which kind of artwork
they clicked gets the right answer either way.

**What it measures:**

| Operation | Time |
|---|---|
| Wand on a four-megapixel photograph | **under 30 ms**, click to cut path |
| Trace of that region, simplified | under 20 000 points, guaranteed |
| Lasso cut on a placed bitmap | under 1 ms |
| Import, decode and place a PNG | 1 ms at 128², 12 ms at 2048² |

23 tools in the palette now, and still exactly two that pick objects: the Lasso
and the Magic Wand sit in their own group because they mark out a *region*
rather than pointing at a thing, and putting them beside Selection would undo
the point of the two-tool rule.

---

### ✅ A soft brush — and a bitmap the GPU stops re-uploading

**A soft edge is the one thing an outline cannot describe.** Every other brush
here makes geometry: a silhouette filled with a colour, which scales to any
zoom and stays editable, and which is the right answer for almost everything. A
stroke that *fades* is not that. It is a different opacity at every point of a
region, and approximating it with concentric bands is what `buzz_fx`'s blur
does — at a boolean per band.

So the soft brush paints pixels, the pixels become a bitmap, and the bitmap is
a fill. Which means the stroke it makes is an ordinary shape: the Lasso cuts
it, the Magic Wand picks from it, a mask holes it, a tween moves it, Convert to
Symbol makes it reusable. Nothing downstream knows it was painted.

**Coverage, one byte a pixel — not colour.** A stroke is one colour, so the only
thing varying across it is how much of that colour each pixel received. Storing
coverage alone is a quarter of the memory and makes the rule that matters
trivial: overlapping stamps within a stroke take the **greater** coverage rather
than adding. Adding is what produces the beaded string of dark blobs that gives
away a naive brush, and it is why Photoshop separates flow from opacity.

**Each stroke is its own bitmap, sized to the stroke.** The alternative — a
stage-sized canvas per layer, as Photoshop has — would cost a full-canvas copy
on every pointer move, eight megabytes at 1080p, sixty times a second, because
the document is copy-on-write and undo holds the state before. A stroke-sized
buffer costs what was painted: a dab is a few kilobytes. What is given up is
painting across strokes, recorded in §7.

**And a defect in the bitmap work committed an hour before it**, found while
building this. The GPU keeps its own copy of every bitmap and decides whether
that copy is still good by comparing an identifier — and `peniko::Blob::new`
takes a fresh number from a global counter on every call. Building the brush
per frame therefore made every frame a cache miss: a four-megapixel photograph
was being re-uploaded sixty times a second for as long as it was on screen. The
identifier now comes from the asset's own id mixed with a generation counter
that moves only when the pixels do. A brush preview, which is genuinely new
pixels every move, opts out explicitly — because with a stable identity the
renderer would correctly believe it already had that picture and go on showing
the first frame of the stroke while the user drew.

**What it measures:**

| Operation | Time |
|---|---|
| Magic wand on a four-megapixel photograph | **19 ms** |
| Soft stroke, full width of a 1080p stage, r=20 | **7.8 ms** (1782×482) |
| A typical dab or short stroke | under 1 ms |
| Photograph on screen, unchanged | **no upload** (was: the whole picture, per frame) |

Twenty tests in `bitmap_tools.rs` drive the pointer rather than the geometry,
and one on the real GPU checks that the fade survives straight alpha, the
unit-square transform, the sampler and the compositor — arriving on screen as a
stroke that fades into the page, monotonically, with no ring and no fringe.

---

### ✅ The panels: one scrollbar a column, and four switches on a layer

Five complaints, four causes, and one of them explained three of the five.

**A panel in a dock column was taking ten thousand points of it.** Every list
panel - Layers, Tools, Depth, Rig, Library - opened a vertical `ScrollArea` of
its own, inside the one the dock column already had, with
`auto_shrink([false, false])`: *fill the space available*. Inside another scroll
area the space available is not the height of the window; egui offers about ten
thousand points. So the Layers panel took all of it, its scrollbar landed on top
of the column's - the overlap that was reported - and **Properties, Colour, the
Library and the Assets panel were pushed ten thousand points down the page**,
where nobody was ever going to find them. The Assets panel had been in the saved
layout all along.

Measured rather than eyeballed: `a_layers_panel_takes_only_the_room_its_rows_need`
put the number at **10 004 points for a single layer row**, and fails at once if
the pattern comes back. The column scrolls; a panel in it does not. The Library
keeps a scroll area of its own, because three hundred symbols should not carry
the whole column with them - but at a **fixed** three hundred points, not at
whatever the column was willing to promise.

**And nine panels in one column is too many even when they fit.** Each is now
collapsible, and the five reached for occasionally - Depth, Rig, Filters,
Lighting, Sound - start rolled up.

**The layer row had one glyph for two switches.** `O` meant *visible*, and `O`
also meant *show as outlines*. They are now drawn rather than lettered, for the
same reason the tool icons are: an eye that strikes through when hidden, a
padlock that opens, and Animate's hollow square that fills in when the layer
draws normally - so the switch shows what the artwork *is*, not what clicking
would do to it.

**Layer transparency**, the fourth switch, is new: a percentage per layer, drag
or type. It rides the same road the onion-skin ghosts use - one alpha
multiplying every colour on the way out - so a dimmed layer needed no second
mechanism, and a dimmed layer seen as a ghost is dimmed by both rather than by
whichever was checked last. **It fades the working view and never the film**:
the flag is on in the stage's options and off in the export's, which
`layer_transparency_fades_the_working_view_and_not_the_export` checks on the
real GPU, both halves. Format version 18, and a document from before it opens
solid rather than invisible.

**The background colour** now has two ways in: the picker, and a hex field. A
picker is a button that opens a popup, and a popup is the one control that can
be present, correct and still unreachable. Six characters in the notation every
palette already uses cannot fail that way. Stroke and fill got the same field.

### ✅ And a defect in the bitmap identity, found by its own tests

The soft-brush work gave every bitmap a stable identity so the GPU would stop
re-uploading unchanged photographs. It was built from the library id and a
change counter - and that is **wrong in a way that does not show until it
does**. Two `ImageAsset`s can carry the same `ImageId` and hold different
pixels: a document opened twice, an import run again, a symbol duplicated, two
tests building the same fixture. Vello's atlas keeps whichever picture it saw
first under an identity and serves it to everyone after.

It surfaced as two bitmap tests failing **intermittently, and only with all six
in the file running together** - enough images resident in one atlas at once.
Eight runs to confirm the fix, five to confirm the cause by reverting it.

The rule it cost: **identity is issued, never derived from something a caller
can duplicate.** `ImageAsset::from_pixels` takes a number from a counter that is
never reused, the field is private so a struct literal cannot forge one, and
copying an asset copies its identity because the pixels really are the same
pixels. The brush preview's special case - an explicit opt-out of caching -
deleted itself: a rebuilt preview is a new picture by definition.

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
| Tests | 1 365 passing, clippy clean |
| Rust source | ~48 000 lines |
| Crates built | 16 of 17 |
| Phases done | 0, 1, 2, 3, 4, **5**, **7** (gaps in §7), plus CP-6.1 and CP-8.1 |
| Format version | 16 — adds gradients |
| Formats heard | `.wav`, `.mp3`, `.ogg`, `.flac`, `.m4a`, `.aac` |
| IK budget | 50 six-bone rigs solved in parallel, well inside one frame |
| Formats read | `.buzz`, `.fla`, `.xfl`, `.swf`, `.pdf`, `.ai` |
| Brush preview frame | 0.57 ms at 6 000 samples and 0.5 spacing |
| Lights | Sun, Sky, Lamp; shading, highlights and cast shadows as vector geometry |

---

## 6. Implementation plan

### ✅ Phase 1 — Geometry & document core — **COMPLETE**
*No UI yet. This is the foundation everything else sits on.*

- [x] **CP-1.1** `buzz-geom` expansion — **complete**
  - [x] **Document-space clipping** (retires the culling limitation, §7)
  - [x] Boolean ops (union, subtract, intersect, xor), parallel tree reduction
  - [x] Path offsetting, simplification, smoothing
  - [x] Parallel hit-testing; stroke hit-testing with tolerance
- [x] **CP-1.2** `buzz-scene` — the document model — **complete**
  - [x] Copy-on-write scene graph (`Arc` structural sharing)
  - [x] **Layer model matching Animate** — see §8.2
  - [x] Groups, transforms, z-order, depth
  - [x] R-tree spatial index (`rstar`), rebuilt off-thread
- [x] **CP-1.3** `buzz-doc` — persistence — **complete**
  - [x] `.buzz` format (zip + JSON), versioned
  - [x] Snapshot-based undo/redo (snapshots *are* the history)
  - [x] Background autosave and crash recovery
- [x] **Exit test:** documents round-trip through disk; undo returns to empty
      and redo restores; a full edit → save → crash → recover cycle passes

### ✅ Phase 2 — Stage, tools & UI shell — **substantially complete** (§7 lists gaps)
- [x] **CP-2.1** Application frame — see §8.1 for the exact layout
  - [x] Menu bar with Animate's menu structure
  - [ ] ~~Dockable panels via `egui_dock`~~ — deliberately not done, see the
        deviation recorded under CP-2.1 in §4; workspace layouts unsaved (§7)
- [x] **CP-2.2** **Stage** — see §8.3
  - [x] Stage rectangle + pasteboard (work area)
  - [x] Document properties: dimensions, background colour, frame rate
  - [x] Rulers, guides, grid, snapping
  - [x] Zoom control with Animate's presets **plus unbounded entry**
  - [ ] Scenes — one per document (§7 item 12)
- [x] **CP-2.3** Toolbar with Animate's tools and shortcuts — see §8.4
- [x] **CP-2.4** Drawing and editing
  - [x] Merge-shape vs object-drawing modes (an Animate-specific behaviour)
  - [x] Strokes, fills, swatches — gradients outstanding (§7 item 8)
  - [x] Free transform, subselection, path editing
- [x] **Exit test:** reproduce a reference Animate drawing tool-for-tool

### ✅ Phase 3 — Timeline & frame animation — **complete**
- [x] **CP-3.1** Timeline panel — see §8.5
  - [x] Layer list + frame grid, playhead, frame numbers, fps, elapsed time
  - [x] Keyframe / blank keyframe / frame span rendering in Animate's style
  - [x] F5 / F6 / F7 / Shift-F5 / Shift-F6 behaviour
- [x] **CP-3.2** Layer types — see §8.2
  - [x] Normal, folder, mask, masked, guide, guided
  - [x] Show/hide, lock, outline view, layer colour, layer depth
- [x] **CP-3.3** Playback
  - [x] Playback decoupled from render rate; loop; frame stepping
  - [x] Scrubbing; onion skinning + outlines
  - [ ] Speculative prefetch across cores, and edit multiple frames — not
        needed yet: scrubbing is already well inside budget without them
- [x] **Exit test:** 500-frame, 20-layer document scrubs at 60 fps

### ✅ Phase 4 — Symbols, library & tweens — **complete** (§7 lists gaps)
- [x] **CP-4.1** Symbols: Graphic, MovieClip, Button; nested timelines;
      instance overrides; symbol editing mode; breadcrumb bar
- [x] **CP-4.2** Library panel: folders, search, usage counts
      *(previews and background-generated thumbnails deferred — §7 item 17)*
- [x] **CP-4.3** Tweens: classic, motion, shape; easing in the model
      *(motion paths, shape hints and the Motion Editor deferred — §7 item 18)*
- [x] **Exit test:** a fixture document with symbols in nested folders,
      instances carrying colour effects, and all four span styles round-trips
      through disk and draws correctly — verified on screen
- **Note:** had to land before Phase 5 — importers need somewhere to put data

### ✅ Phase 5 — Importers — **complete** (§7 lists gaps)
- [x] **CP-5.1** `.fla` / `.xfl` — unzip, parse `DOMDocument.xml` +
      `LIBRARY/*.xml`, map onto the Phase 4 model, layer folders, fill and
      stroke style tables, tweens, and a fidelity report
  - [ ] `bin/` media extraction and parallel per-symbol parsing — deferred:
        media needs the Phase 6 pipeline, and parsing is not a bottleneck yet
- [x] **CP-5.1b** Merge into an open document, remapping every id (§4)
- [x] **CP-5.2** `.pdf` / `.ai` — content-stream path extraction via `lopdf`
      rather than `pdfium-render`, for the reason recorded in §4; `.ai` v9+ is
      PDF internally so one parser covers both, and pre-v9 PostScript fails
      with a clear message
- [x] **CP-5.3** `.swf` — `DefineShape` / `DefineSprite` / `PlaceObject` →
      editable vectors and library symbols
  - [ ] Bitmaps and fonts — reported, not read; both need subsystems that do
        not exist yet (§7 items 9 and 22)
- [x] **Exit test:** all three formats read, merge into a document that already
      has artwork and a colliding name, and round-trip through disk
  - [ ] Frame-by-frame comparison against an Animate reference render — **not
        done**, and not possible here; see §7 item 21

### 🟡 Phase 6 — Export — **CP-6.1 done**
- [x] **CP-6.1** PNG image and PNG sequence — any resolution, transparent
      background optional, encoded across every core, on a background thread
      with progress and cancel (§4)
- [ ] **CP-6.2** MP4 / MOV — NVENC (`h264_nvenc` / `hevc_nvenc` / `av1_nvenc`)
      via `ffmpeg-sidecar`, N frames in flight
- [ ] **CP-6.3** GIF / WebP — palette quantisation, animated WebP
- [ ] **CP-6.4** HTML5 Canvas / SVG — scene graph → JS + small runtime player
- [ ] **Exit test:** one document exported to all four, all four play correctly

### ✅ Phase 7 — Rigging — **COMPLETE** (gaps in §7)
- [x] **CP-7.1** Bone tool, armatures over shapes and symbol chains
- [x] **CP-7.2** FABRIK IK solver with angle limits and pin constraints
- [x] **CP-7.3** Puppet warp via Moving Least Squares mesh deformation
- [x] **CP-7.4** Vertex weight binding, armature tweening
- [x] **Exit test:** `buzz-app/tests/rig_exit_test.rs` rigs a character arm
      through the editor's own gestures, keys two poses, tweens between them,
      saves and reopens the document, and exports the result — plus 50
      six-bone rigs solved in parallel inside one frame at 24 fps

### 🟡 Phase 8 — Scripting & ActionScript — **CP-8.1 done, CP-8.2 started**
- [x] **CP-8.1** Plugin API (JSFL equivalent) — `rquickjs` sandbox exposing
      document / timeline / library / selection, with a time, memory and stack
      budget so a script cannot hang or exhaust the editor
  - **Changed from the plan:** scripts mutate a working copy rather than
    submitting through a command queue, because a queue cannot answer a read
    after a write. One run is still one undo step — see §4.
  - [ ] Running off the UI thread — deferred: a 5-second budget bounds the
        stall, and `Scene` is not `Send` through the engine's `Rc` handles, so
        this needs the run to own its scene outright
- [ ] **CP-8.2** Actions panel — the panel, the Output area, F9, Ctrl+Enter and
      runnable examples are in (§4)
  - [ ] `tree-sitter` highlighting, autocomplete, error squiggles
  - [ ] Frame scripts and symbol class linkage — needs somewhere in the
        document to keep a script, which is a format change (§7 item 30)
  - [ ] Saved commands in the Commands menu, as Animate's JSFL files are
- [ ] **CP-8.3** AS3 runtime — embed `ruffle_core`'s AVM2 (MIT/Apache)
- [ ] **CP-8.4** AS3 compiler — **bundle Apache Royale/Flex `mxmlc`** first
      (weeks, costs a bundled JRE) rather than writing AS3→ABC from scratch
      (6–12 months). Native Rust ABC emitter later to drop the Java dependency.
- [ ] **Exit test:** frame script drives a MovieClip; project publishes and runs

---

## 7. Known issues & deviations

**Every iteration lands here.** This file is the project's record: each piece of
work gets a section in §4 saying what was built and *why it was built that
way*, and every deviation from Animate — or from what a reasonable person would
expect — gets a numbered row below, with a status. A change that is not written
down here has not been finished.


| # | Item | Status |
|---|---|---|
| 1 | ~~**Oversized paths culled, not clipped.**~~ | ✅ **Resolved in CP-1.1** by `RenderClip` |
| 8 | ~~**Gradients not implemented.**~~ | ✅ **Resolved** — linear and radial gradients on fills and strokes, a working Gradient Transform tool, format version 16 |
| 9 | **Text tool not implemented.** Needs font loading, shaping and a text-editing caret — a subsystem in its own right. | Phase 2 follow-up |
| 10 | ~~**Lasso tool not implemented.**~~ | ✅ **Resolved** — freehand region that *cuts* the artwork it crosses, plus a Magic Wand beside it |
| 11 | **Pen tool draws line segments, not Bézier curves.** Click-drag handle authoring is not there yet; anchors can be edited afterwards with Subselection. | Phase 2 follow-up |
| 12 | **Multiple Scenes not implemented.** One scene per document. | Deferred |
| 15 | ~~**Tweening not implemented.**~~ | ✅ **Resolved in CP-4.3** — classic, motion and shape tweens interpolate in the render path |
| 17 | **Library has no previews.** Symbols are listed by name, kind and use count; there is no thumbnail. Needs off-thread rasterisation into a cache keyed by symbol and revision. | Phase 4 follow-up |
| 18 | **No Motion Editor, motion paths or shape hints.** Easing exists in the model (strength and cubic Bézier) and interpolates correctly, but nothing in the UI edits a curve, and a motion tween cannot yet follow a drawn path. | Phase 4 follow-up |
| 19 | ~~**Import commands are not wired.**~~ | ✅ **Resolved in CP-5.1b** — `Scene::merge` remaps every id; all three formats are on the File menu |
| 20 | ~~**The XFL importer does not restore folder nesting.**~~ | ✅ **Resolved in CP-5.1c**, along with two fidelity bugs it exposed |
| 21 | **No importer has been checked against a real file from Adobe.** Every fixture is one we wrote, so the importers are verified against the *specifications* and against files whose content we chose — not against what Animate, Illustrator and the Flash compilers actually emit, which is where the awkward cases live. This is the largest single risk in Phase 5. | Needs a licensed Animate/Illustrator and real-world files |
| 22 | ~~**Bitmaps are not imported.**~~ | ✅ **Resolved** — `Paint::Image` rather than a bitmap object kind, File ▸ Import Image, `media/` storage, format version 17. The three *importers* still do not carry their bitmaps across — see item 158 |
| 23 | **SWF morph shapes, buttons, filters, blend modes and colour transforms on placements** are reported but not applied. Colour transforms are the cheapest of these to fix — the model already has `ColorTransform` — and would noticeably improve fidelity. | Phase 5 follow-up |
| 28 | ~~**No 3D object rotation.**~~ | ✅ **Resolved** — `Spatial` on every object, rendered through its own plane, format version 11 |
| 63 | **3D rotation is allowed on every object.** Animate restricts it to movie clip instances, because there it is a property of a display object with a cached surface. Here it is a plane in a projection and costs nothing extra, so a shape, a group or a rigged character may have it too. | By design |
| 64 | **A turned object is set by numbers, not by a gimbal.** Animate drags coloured rings on the stage; here the angles are sliders in the Properties panel. The model is the same; what is missing is the on-stage widget and its drag. | Follow-up |
| 65 | **Turned objects do not sort by depth against each other.** Paint order is still the timeline's, so two cards that cross in space draw one wholly in front of the other. Correct sorting needs a depth buffer or a per-frame sort, and the timeline being the authority on what is in front is the rule everywhere else here. | By design |
| 60 | **A tilted camera does not sort layers by depth.** Paint order is the timeline's, exactly as it is without tilt, so two layers that would cross in space still draw one wholly in front of the other. Animate has the same rule and for the same reason: the timeline is the authority on what is in front. | By design |
| 61 | **Free Transform handles on a tilted camera hang off the quad's extent**, not off the quad. The selection outline is drawn as the true quadrilateral, but the eight handles sit on its bounding box — a transform gizmo that lives in perspective is a piece of work in its own right. | Follow-up |
| 62 | **A soft-edged filter under a tilted camera uses a flat pen.** The bands of a shadow or glow are strokes, and Vello strokes under an affine only, so their width does not widen towards the viewer. Visible only on a steeply tilted layer with a large blur. Fixing it means outlining every band into a fill and paying a boolean for each. | Follow-up |
| 29 | **Depth does not blur.** Animate's camera has a depth-of-field effect; layers off the focal plane are sharp here however far away they are. Needs a blur in the render path. | Follow-up |
| 27 | **Build-up paint is a deliberate deviation from Animate**, which has no such mode: its shapes always composite source-over. It is off by default, so a document that does not ask for it behaves exactly as Animate would. Added because overlapping translucent strokes that deepen is what a brush *should* do, and because the request was explicit. | By design |
| 25 | **Pen pressure is plumbed through but never supplied.** The brush reads `StrokeSample::pressure` and the setting is in the panel, but winit 0.30 gives no tablet pressure on Windows, so every sample arrives at 1.0 and the pressure option paints a constant width. Speed is the default response for exactly this reason. Needs a platform tablet backend (Windows Ink / Wintab). | Brush follow-up |
| 26 | **Brush strokes do not merge with what is under them.** Each stroke is its own shape even in Merge Shape mode; Animate would fuse same-coloured overlapping paint. The booleans exist (CP-1.1b) — this is a matter of routing brush output through them, which was left out because a boolean per stroke would undo the responsiveness work unless it is done off the interactive thread. | Brush follow-up |
| 24 | **PDF clipping paths are ignored.** `W`/`W*` are recorded in the report but not applied, so artwork that a real file clips away arrives whole. Needs a clip concept in the scene model, which nothing else has wanted yet. | Phase 5 follow-up |
| 38 | **Sound has no Properties panel.** Attaching a sound puts the newest import on the current keyframe with Animate's Stream sync; there is no picker, no per-sound volume or effect, and no way to choose Event/Start/Stop from the interface. The model carries all four sync modes and a volume — nothing edits them yet. | Sound follow-up |
| 39 | **Only Stream sync actually differs.** Event, Start and Stop are stored, saved and reported, but the player treats every cue as timeline-positioned. An Event sound that should carry on past its keyframe stops with the playhead. | Sound follow-up |
| 40 | **Resampling is nearest-neighbour.** A 44.1 kHz file on a 48 kHz device plays in the right place and at the right length, but the pitch is not exactly right and there is aliasing on bright material. Fine for animating to dialogue; not fine for a finished mix. | Sound follow-up |
| 41 | **No sound in exports.** PNG sequences have no audio by definition; video export (CP-6.2) is where a soundtrack has to be muxed in, and that is not built yet. | Phase 6 |
| 42 | **Lip sync is signal analysis, not phoneme recognition.** It distinguishes silence, open and rounded vowels, and fricatives; it cannot distinguish `p`/`b`/`m` or `l`/`n`, which land on the closed and tongue shapes. Animate uses a trained model here. Recorded as a limitation rather than sold as parity. | By design |
| 43 | **Scripting cannot reach sound.** `fl.getDocumentDOM()` exposes no sounds, no lip sync and no playback, so none of this can be driven from the Actions panel. | Phase 8 follow-up |
| 33 | **An armature is an object, not an armature layer.** Animate moves rigged artwork onto its own layer whose keyframes are poses, and refuses to let you draw there. Here a rig is an object on an ordinary layer, which is why keyframes, tweens, undo, symbols and importing all work on it with no second code path — but the timeline does not mark the layer, and nothing stops you drawing on it. Recorded as a deviation, with the reason, in §4. | By design |
| 34 | **No Bind tool.** Weights are computed at bind time and re-computed when the skeleton changes; there is no way to paint them by hand, which is Animate's Bind tool. A limb whose weights are wrong must be re-rigged rather than corrected. | Phase 7 follow-up |
| 35 | **Bones cannot be deleted or reparented** once drawn, and there is no way to move a joint without posing it. Building a rig is currently additive. | Phase 7 follow-up |
| 36 | **Joint speed is not implemented.** Animate gives each joint a speed that damps how much of a drag it absorbs; every joint here responds equally. | Phase 7 follow-up |
| 37 | **F6 past the end of a span makes a *blank* keyframe.** Animate extends the span and duplicates the previous artwork; here there is no frame to duplicate, so the keyframe comes up empty and the artwork appears to vanish. Working around it means pressing F5 first. Found while writing Phase 7's exit test. | Phase 3 defect |
| 44 | **Lighting is a deliberate departure from Animate**, which has no lights at all. It is off until a light is added, and a document with none renders pixel-identically to one from before the feature existed — so nothing an Animate user expects is changed by its existence. Added because it was asked for by name, and built the way Blender presents it. | By design |
| 45 | **One key light does the modelling.** Shading and cast shadows follow the strongest directional light; every other light contributes colour and fill only. Summing crescents from several lights is physically nicer and reads as dirt on flat artwork. | By design |
| 46 | **Lighting is geometry, not shading.** Vello offers no shader hook, so a shaded side is a crescent cut by a boolean and a cast shadow is a projected silhouette. It keeps everything editable and zoomable; what it cannot do is a gradient falloff across a single fill, a self-shadowing fold, or light through a translucent shape. | By design |
| 52 | **Filters are geometry, not a raster pass**, for the reason §7 item 46 gives for lighting. A real blur mixes a shape with what is *inside* it; these build from the outline, so a two-colour drawing blurs each shape against its own edge rather than into its neighbour. Very close for flat vector artwork, which is what this program makes. | By design |
| 53 | **Filters can go on any object and on a layer.** Animate allows them on movie clips, buttons and text only, because a raster filter needs a surface to cache. There is nothing to cache here, so the restriction would be arbitrary — and "blur the background layer" is a thing animators ask for constantly. | By design |
| 54 | **Four blend modes are missing**: Subtract, Invert, Alpha and Erase. They are Flash's own compositing operators rather than the PDF/CSS mixing modes — Alpha and Erase use the *parent* clip's alpha as a mask, which is a compositing model rather than one equation. Left out rather than mapped onto something that looks nearly right. | Needs a compositing model |
| 55 | **Gradient Glow and Gradient Bevel are still not implemented**, though gradients now exist (§7 item 8). Both are the plain Glow and Bevel with a ramp instead of a colour; what is missing is the ramp reaching the filter's band geometry, which builds its colours per band rather than taking a paint. | Follow-up |
| 50 | **A followed layer's motion is read off its first object.** A layer has no transform of its own; Animate tracks one for the layer itself. Here the first object on the layer leads, which is exact for the one-symbol-per-layer rigs layer parenting exists to serve, and approximate for a layer of loose artwork. | By design |
| 51 | **Artwork on a followed layer is edited in its own space.** Clicking and marquee selection are mapped through the inherited transform, so a parented limb is clickable where it is drawn; a *drag* is still applied in the layer's own frame, so dragging artwork whose parent is rotated or scaled moves it along the layer's axes rather than the parent's. Layer depth has the same limitation, from the same cause. | Follow-up |
| 47 | **Lights are not keyframed.** The rig belongs to the document, not to the timeline, so a sun cannot swing across a shot the way a camera can pan. Needs the light rig on the same tween path the camera already uses. | Lighting follow-up |
| 48 | **A light gizmo is grabbed only with the Selection tool.** On-stage handles belong to Selection the way transform handles do; a lamp sitting over the canvas must not swallow a brush stroke aimed at the artwork beneath it. | By design |
| 49 | **Scripting cannot reach the lights**, as it cannot reach sound (§7 item 43). `fl.getDocumentDOM()` exposes no rig, so lighting cannot be driven from the Actions panel. | Phase 8 follow-up |
| 30 | **A script lives in the panel, not in the document.** There are no frame scripts and no saved commands: what is typed in the Actions panel is view state, so it is not saved with the `.buzz` file and does not survive closing it. Both need somewhere in the format to keep a script, and frame scripts additionally need the player to run them at the right frame. | Phase 8 |
| 31 | **The scripting API is a useful subset, not JSFL.** Rectangles, ovals, layers, frames, selection, the library and document properties are there; text, gradients, tweens, groups, transforms beyond translation, and `fl.fileSystem` are not. Several of those are gaps in the editor itself (§7 items 8 and 9) rather than in the binding. | Phase 8 follow-up |
| 32 | **A script runs on the UI thread**, so a five-second script freezes the window for five seconds — bounded, but visible. Moving it off needs the run to own its scene outright, since the engine holds it behind `Rc`. | Phase 8 follow-up |
| 16 | **Camera rotation and zoom have no direct gesture.** Both are keyable and interpolate correctly, and `zoom_camera` exists, but only panning is bound to a drag. | Phase 3 follow-up |
| 13 | **Clipboard (cut/copy/paste) not implemented.** Duplicate works. | Phase 2 follow-up |
| 14 | ~~**Workspace layout is not persisted** across runs.~~ | ✅ **Resolved** — panels dock, float and lock, and the arrangement is saved between runs |
| 59 | **The camera row is drawn by the timeline, not stored as a layer.** Animate presents its camera as a layer; here it is the `CameraTrack`, shown as a row. Everything that walks layers is therefore spared having to skip it, at the cost of the row not appearing in the Layers panel, which lists real layers only. | By design |
| 56 | **Panels are moved by menu, not by dragging them.** Animate lets you drag a panel by its title bar and drop it into a dock, with a highlight showing where it will land. Here the same moves are on each panel's own menu. The model underneath is the same; what is missing is the drag, the hit-testing of drop zones, and the preview. | Follow-up |
| 57 | **Panels cannot be grouped into tabs.** Animate stacks several panels in one frame with tabs along the top; here they stack vertically down a side. | Follow-up |
| 58 | **One workspace, not several.** Animate saves named workspaces and switches between them. There is one layout here, plus Reset. | Follow-up |
| 66 | **The looping section is a deviation from Animate.** Animate's loop is a transport setting and never reaches the published file; this one is in the document and the exporter repeats it. It is off by default and a document without one exports byte-for-byte what it always did, so nothing an Animate user expects is changed by its existence. Added because "even in the final render that section keeps looping" was the request. | By design |
| 67 | **One looping section per document, and it does not nest.** A section cannot contain another, and a layer cannot loop on its own while the rest of the timeline runs straight. Both are real requests; both need the playlist to become a tree rather than a range, and every frame lookup to go through it. | Follow-up |
| 68 | **Sound is not repeated with the picture.** Playback seeks the audio back when the section wraps, so it stays in step, but an export writes no audio at all (§7 item 41) and nothing stretches or repeats a soundtrack to match a looped picture. Arrives with video export. | Phase 6 |
| 69 | **A looping section is not marked on the frames themselves**, only on the ruler. Animate has no equivalent to mark, but a band across the frame grid would read better on a tall timeline where the ruler has scrolled out of sight. | Follow-up |
| 70 | **Auto Keyframe is a deviation from Animate**, which has no such mode: you press F6 yourself, or your change reaches back to the start of the span. It is off by default and changes nothing when off. Added because it was asked for by name, and because the alternative is a surprise every time an animator edits inside a span. | By design |
| 71 | **Auto Keyframe does not key a layer the edit does not touch.** Moving a parented limb keys the limb's layer, not the parent's; a camera move is keyed by the Camera menu as before. Animate has no equivalent to compare against. | By design |
| 72 | **There is no arrow-key nudge.** Selected artwork is moved by dragging or by Free Transform; Animate moves it a pixel per arrow press and eight with Shift. Noticed while driving Auto Keyframe from a script. | Phase 2 follow-up |
| 73 | **The onion markers are numbers, not brackets on the ruler.** Animate draws two draggable markers over the frame numbers and offers Onion 2/5/All from a menu; here the transport carries two counts and an All button. The model is the same range; what is missing is the drag and the drawn brackets. | Follow-up |
| 74 | **Edit Multiple Frames does not move keyframes themselves.** It changes the artwork on every keyframe in range; Animate's mode also lets you cut and paste a whole span of frames elsewhere on the timeline. Moving frames as frames is its own feature and is not built. | Follow-up |
| 75 | **Under Edit Multiple Frames the artwork of other keyframes draws in paint order, not in time order.** Two drawings that overlap on the stage are stacked by layer, so which one appears in front does not follow which frame it belongs to. Animate has the same behaviour. | By design |
| 76 | **Named swatches in folders are a deviation from Animate**, whose Swatches panel is a flat grid of unnamed chips with `.clr` import. The grid is here as a view; the names and folders are additions, because a production palette is a set of decisions and a decision identified only by its hex value is picked wrongly at four in the morning. | By design |
| 77 | **A swatch is a colour, not a style.** Changing a swatch does not repaint the artwork that used it — the colour was copied into the shape when it was drawn. Animate behaves the same way. Live colour styles would need shapes to reference the palette, which is a different model. | By design |
| 78 | **No `.clr`, `.act` or `.ase` palette import or export.** Animate reads Flash and Photoshop palettes; nothing here does, so a palette from another tool is retyped. | Follow-up |
| 79 | **Swatches are not draggable between folders.** A dropdown per row moves one, exactly as the Library moves a symbol, for the same reason: the drag is a piece of work in its own right. | Follow-up |
| 80 | **The Assets panel is a deviation from Animate's**, which ships a curated set of animated characters and props and syncs with Creative Cloud Libraries. This is the same idea with none of the service: a folder on this machine, holding `.buzz` documents. Nothing is bundled, and nothing is uploaded anywhere. | By design |
| 81 | **Assets have no thumbnails**, for the same reason the Library has none (§7 item 17): rendering a preview needs off-thread rasterisation into a cache. An asset is identified by its name and its folder. | Follow-up |
| 82 | **A placed asset lands where it was drawn**, not under the pointer. Animate drops one at the centre of the stage or where you drag it; here the artwork keeps the coordinates it had when it was kept. Placing then dragging is one extra gesture. | Follow-up |
| 83 | **The assets folder is not watched.** Adding a file outside the application shows up after the panel's refresh button, not immediately — a file watcher is a thread, a platform API and a class of bug for something a button does. | By design |
| 84 | **An asset carries its sounds and lights, but not the stage.** `Scene::extract` takes the objects and the symbols they need; frame rate, stage size, camera and lighting stay with the document being placed into, which is what "place a prop" should mean. | By design |
| 85 | **A symbol's registration point is still inert.** `Symbol.registration` is stored, saved and carried through import and duplication, but nothing edits it and the renderer does not read it — convert-to-symbol rebases the artwork so the registration sits at the origin, and after that the field does nothing. The *object* transformation point is the one that now works. | Follow-up |
| 86 | **There is no live preview while transforming.** A rotate or skew is applied on release, as scaling always was; Animate redraws the artwork as you drag. The maths is the same either way — what is missing is drawing the in-progress transform. | Follow-up |
| 87 | **The transform handles still hang off the bounding box.** §7 item 61 already recorded this for a tilted camera; it applies to a rotated object too, so the eight handles sit on the axis-aligned extent of a turned rectangle rather than on its corners. The transformation point itself is drawn where it really is. | Follow-up |
| 88 | **Skew is not constrained.** Animate holds the opposite edge fixed while shearing; here the shear is about the transformation point, which is the same thing when the point is on that edge and a different thing when it is not. Shear is clamped at 20:1 so a stray drag cannot flatten artwork into a line. | By design |
| 89 | **Two themes, not a theme editor.** Animate's Preferences offer four interface brightnesses and a separate stage colour; here there is Dark and Light, and the stage colour is the document's own. | By design |
| 90 | **The theme is a process-wide atomic**, not a value carried through the drawing code. One window, one UI thread; a second window in one process would share the setting. | By design |
| 91 | **Artwork colours are untouched by the theme**, including onion-skin ghosts and light gizmos, which are drawn in ink against the stage rather than against the chrome. A very dark drawing on a dark stage is as hard to see in either theme — that is the document's business. | By design |
| 92 | **Shape recognition happens on command, not as you draw.** Animate also recognises a shape the moment the pencil is released, controlled by a Preferences setting per shape kind. Here it is Straighten and Recognise Shape. Recognising as you draw needs the setting, and the decision that a stroke has *ended*, which the brush's smoothing pipeline does not currently expose. | Follow-up |
| 93 | **Triangles and polygons are not recognised**, and neither is Animate's "connect lines" — two strokes that nearly meet stay two strokes. Circles, ovals, squares, rectangles at any angle, and lines are what an animator draws roughly and wants tidied. | By design |
| 94 | **A recognised shape replaces the path, not the object.** Fill, stroke, filters, blend and the transformation point are all kept; what changes is the geometry. An open stroke recognised as a line therefore stays a stroke rather than becoming a line *object*. | By design |
| 95 | **The stage zoom control does not follow the panel layout.** It is an overlay pinned to the stage's top-right corner and cannot be docked, moved or hidden, unlike every panel. It is chrome for the stage, in the way the rulers are. | By design |
| 96 | **The New Document dialog is not Animate's.** Animate offers document *types* (ActionScript, HTML5 Canvas, WebGL, AIR) with platform-specific defaults; there is one kind of document here, so the dialog asks the three things that actually differ. | By design |
| 97 | **A new document does not warn about unsaved changes.** File ▸ New replaces what is on screen once the dialog is answered; there is no "save first?" prompt, here or on Open. It is the same gap in both, and needs a modal the shell does not have yet. | Follow-up |
| 98 | **The remembered setup is per machine, not per project.** A folder of documents at one size and another folder at a different one still share the one default. | By design |
| 99 | **A hard kill is covered by the pause, not by the panic hook.** `TerminateProcess`, a power cut or an OOM kill run none of our code; what is on disk is whatever the last pause wrote, so at most a few seconds of continuous drawing is at risk. The panic hook covers the crashes a Rust program actually has. | By design |
| 100 | **Recovery is per directory, not per document.** The prompt scans the application's recovery directory and the last eight directories documents were opened from or saved to. Work saved somewhere else, on a machine that has since forgotten that folder, is not offered — the file is still there, under `…recovery.buzz`, beside the document. | By design |
| 101 | **There is no autosave interval setting.** Animate's Preferences offer one; here it is two minutes, or five seconds of not drawing, and neither is exposed. The writes are small and off-thread, so the cost of the aggressive setting is not worth a control. | By design |
| 102 | **A recovered document does not remember what it was.** It opens untitled, so Save asks where to put it; the original document, if there was one, is untouched on disk. Animate reopens the recovery in place of the document. Deliberate: the recovery is evidence of a crash, not a file the user chose to keep. | By design |
| 103 | **The application icon is a character from another production**, lettered BA. It was replaced with an abstract program mark and then put back on the studio's instruction: whose program this is *is* the point, and the mark it shares with the show is the point of the mark. Noted rather than open — a deliberate choice, not an oversight. | By design |
| 103a | **BA is unreadable below about 24 pixels when it is under the head.** The 16-pixel drawing drops the head and keeps the letters, so the two sizes of this icon are not the same picture. Deliberate, and the alternative — one picture, illegible at the small end — is worse. | By design |
| 104 | **The `.ico` is written by hand rather than by a tool.** `System.Drawing` only makes single-size icons, so the PNG sizes are packed into the ICO container directly. It is the documented format and every size opens correctly; it is worth knowing it is not the output of an icon editor. | By design |
| 105 | **The brand band is three points tall and not configurable.** It sits under the title bar, above the menu, drawn in a foreground layer over everything. Thicker read as decoration; thinner disappeared. | By design |
| 106 | **The launcher is Windows only.** `BuzzAnimate.bat` and the shortcut maker are batch files; on other platforms it is `cargo run --release -p buzz-app`, which the README gives. | Follow-up |
| 107 | **An opened `.fla` becomes an untitled document.** Animate reopens its own file in place; here the translation opens unsaved, so Save asks where to put it. Writing `.fla` back is not possible — this program cannot produce one — and quietly holding the path would invite an overwrite that destroyed the original. | By design |
| 108 | **The stage scrollbars have no arrow buttons and do not page.** Dragging the thumb and clicking the track both move the view; clicking beside the thumb jumps there rather than stepping a page. | By design |
| 109 | **The scrollable extent is recomputed every frame** from the artwork on the current frame. It is a bounding-box union over the visible objects, which is cheap at the scale documents reach here and would want caching at a hundred thousand objects. | Follow-up |
| 110 | **The docks resize from our own splitters, not egui's.** `Panel::resizable` is off and the size comes from the workspace, so a panel cannot be resized by any means egui offers — including double-clicking an edge to collapse it, which egui's own handle supports and this does not. | By design |
| 111 | **The document's length is one number for every layer.** Animate has no global length either — the film is as long as its longest layer — so setting it extends the short layers and trims the long ones. A layer that was deliberately shorter than the rest loses that difference when the length is set. | By design |
| 112 | **The tool strip flows by width, not by a column count.** There is no "one column / two columns" setting: it fits what it can, which is what makes dragging the dock's edge do something useful. | By design |
| 113 | **The frame clipboard is one frame, not a span.** Animate's Cut/Copy/Paste Frames work on a *selected range* of frames; the timeline here has no span selection, so these act on the frame the playhead is on. The commands and their keys match Animate; the scope does not. | Follow-up |
| 114 | **Reverse Frames reverses the whole layer.** Animate reverses the selected span; with no span selection, this reverses every keyframe on the active layer. | Follow-up |
| 115 | **An imported Animate asset is a document, not an Animate asset.** The manifest's keywords, category and Animate's own thumbnails are not carried across — what arrives is the artwork, filed by role and subcategory. Searching by keyword is Animate's; searching by name is ours. | By design |
| 116 | **Bitmap assets do not come across at all.** They are counted and reported as skipped. This is §7 item 22 — no reader here imports bitmaps — and it is the one thing that stops an Animate library arriving whole. | Blocked on §7 item 22 |
| 117 | **The imported camera is linear between keys.** Animate eases its camera with the same tween controls it gives artwork; ours interpolates straight from key to key, so an eased camera move arrives with even timing. The positions and zooms are exact. | Follow-up |
| 118 | **A damaged symbol is truncated, not repaired.** When Animate has written malformed XML, everything before the damage is kept and everything after it is lost — the reader cannot know what the rest was meant to say. It is named in the report so the symbol can be re-saved from Animate. | By design |
| 119 | **The inverse mask is ours, not Animate's.** A `.fla` has no way to express it, so a document using one exports and saves correctly here and cannot be round-tripped through Animate. Format version 15 records it. | By design |
| 120 | **An inverse mask is bounded by what it hides.** The group is a render target, sized to the masked layers' artwork plus a margin; artwork that reaches further than that margin — a very wide blur under an inverse mask — would be clipped at the edge of the group. | Follow-up |
| 121 | **The timeline's zoom is not Animate's menu.** Animate offers Tiny/Small/Normal/Medium/Large and Short/Normal/Tall; these are two sliders over the same ranges. The names are gone; the sizes are continuous. | By design |
| 121a | **The `.swf` road in is far behind the `.fla` one.** A published SWF carries geometry Animate has already resolved, and the reader is Ruffle's — so it should be the easier path. It is not yet: a sample SWF imports its instances and draws nothing, because the SWF importer stopped at Phase 5 and the XFL one has had a rewrite since. Worth finishing, and not worth recommending today. | Follow-up |
| 122 | ~~**Long thin strays cross some imported frames.**~~ | ✅ **Fixed** — they were unsigned hex coordinates: one point per shape at eight hundred thousand pixels, drawing a spike from the artwork to it. Signed now, and the strays and the stretched limbs went together. |
| 123 | **A rig is baked, not linked.** The parent's rest pose is multiplied into the child at import, so editing the parent's *rest* pose afterwards moves the parent alone. Animate keeps the relationship live. The rig animates correctly; it is re-rigging that would need the link. | By design |
| 124 | **`rigPropagationMatrix` is ignored.** Animate stores how much of a parent's transform reaches each child; here all of it does. A rig that leans on partial propagation will arrive stiffer than it was. | Follow-up |
| 125 | **A graphic's loop is resolved from its layer's keyframe**, not from when the instance itself appeared. They differ when a keyframe holds several instances placed at different times — rare, and the pose is then one cycle out. | Follow-up |
| 126 | **The transformation point can be moved with the Selection tools, which Animate does not allow.** Animate reserves it for Free Transform (Q). Here a *drag* that starts on the circle moves it from the Selection and Subselection tools as well; a click still selects. A deviation, on the grounds that changing tools to move a pivot and changing back is a step for nothing. | By design |
| 127 | **Edit in place does not move the symbol under its instance.** Animate draws the opened symbol where the instance sits; here it is drawn at the symbol's own origin with the scene paled behind. The context is right, the registration is not. | Follow-up |
| 128 | **The Gradient Transform tool has one grip where Animate has two.** Animate puts scale and rotation on separate handles a few pixels apart on the same line; here dragging the end of the ramp does both, so the ramp's end goes where the pointer is. One grip instead of two adjacent ones, and never a question of which was grabbed. | By design |
| 129 | **A gradient under a tilted camera is placed by an approximated affine.** A brush transform is a matrix and a perspective projection is not. Where the projection is affine — every document that has not tilted its camera — the placement is exact; where it is not, an affine is fitted to three corners of the shape's own bounding box mapped through the real projection, which is exact at those points and close between them. Same class as §7 item 62. | Follow-up |
| 130 | **A gradient under a blur falls back to one colour.** `buzz_fx::blur_ops` builds its soft edge from bands, each a stroke of a single colour, so a blurred gradient-filled shape blurs its average. Fixing it means the band geometry taking a paint rather than a colour — which is also what §7 item 55 needs. | Follow-up |
| 131 | **A hairline stroke cannot carry a gradient.** Its width is one pixel at every zoom, so it is set in screen space and does not go through the paint path. One pixel of a ramp is one colour, so what is lost is a ramp *along* the line. | By design |
| 132 | **The eyedropper samples a gradient to one colour.** Animate's picks up the whole ramp and loads it into the Color panel; here the colour well takes the ramp's average. The gradient is still on the artwork — what is missing is copying it from one shape to another. | Follow-up |
| 133 | **The brush and pattern previews are drawn in one colour.** They are egui chrome, redrawn on every pointer move, and egui's painter has no gradient brush; the committed stroke gets the real gradient. What the preview is for — spacing and stamp size — is unaffected. | By design |
| 134 | **PDF gradients are still flattened.** PDF expresses them as shading dictionaries and Pattern colour spaces, which is a subsystem rather than an attribute — seven shading types, of which two are function-driven meshes. The XFL road brings gradients in; the PDF one still averages them. | Phase 5 follow-up |
| 135 | **A gradient and a solid do not tween into each other.** Two gradients interpolate stop by stop when they correspond; a solid tweening to a gradient switches at the halfway point instead, because moving a colour and then jumping to a ramp reads as a glitch rather than a transition. Two gradients with different stop counts switch for the same reason. | By design |
| 153 | **An armature layer is not marked, so a rigged film looks like any other.** §7 item 33 already records that a rig is an object rather than an armature layer; building the horror short confirmed the cost — with thirty rigged characters there is nothing in the timeline that says which layers hold rigs. | Phase 7 follow-up |
| 154 | **Lighting geometry is single-threaded.** Booleans, hit-testing, IK and export encoding all use the cores; shading crescents and cast shadows are built one shape at a time. It does not matter yet — the first lit frame of a heavy scene is 305 ms and every frame after it is 6 ms — but it is the largest remaining single-threaded cost in the draw walk. | Follow-up |
| 155 | **The first frame of a heavy scene costs 305 ms**, because every crescent, shadow and blur is built before anything appears. It is paid once and then cached, but opening a complex document shows nothing for a third of a second. Building the geometry on the background pool and drawing unlit until it arrives would hide it. | Follow-up |
| 157 | **A mask added before the layer it masks ends up underneath it and claims nothing**, and the sheet it should have holed then draws flat across the whole film. Masking is positional and `add_layer` puts each new layer in front, so the masked layer has to be created *first*. Animate's order of work is the same — draw the content, then add a mask above it — but nothing here says so, and the symptom (an evenly darkened film) looks like a lighting problem rather than a layer-order one. | Follow-up |
| 156 | **A layer that is one frame long shows nothing past frame zero**, which is correct and is also a trap: setting a document's length is a separate action (`set_frame_count`, Animate's F5), and a scene built without it appears to lose all its artwork the moment the playhead moves. Nothing in the interface says so. | Follow-up |
| 158 | **The XFL, SWF and PDF readers still do not bring their bitmaps across.** The pipeline they needed now exists — decode, library, `media/` storage, a paint that draws them — so each reader has only to call it where it currently logs "bitmap ignored". Doing so needs the DefineBits/JPEGTables chain for SWF and the `<DOMBitmapItem>` road for XFL. | Phase 5 follow-up |
| 159 | **The Magic Wand has its own letter, `G`.** Animate has no letter for it, because there it is a mode of the Lasso in the property inspector rather than a tool. Here it is its own tool with its own icon, so it takes the free letter next to the Lasso's `L`. Every other letter follows Animate exactly. | By design |
| 160 | **An imported picture larger than the stage is scaled to fit it.** Animate places at natural size, so a phone photograph lands mostly off the pasteboard and the first thing anyone does is scale it down by hand. Aspect ratio is kept and one Free Transform drag undoes it. A picture that already fits is placed untouched, so pixel-exact artwork stays pixel-exact. | By design |
| 161 | **The wand's traced region is polygonal, not curved.** The boundary of a flood fill is a staircase; simplification straightens it, and nothing refits Béziers to it afterwards. At any sane zoom the difference is invisible, and a curve fit would have to decide which corners are real — a photograph's edge has both kinds. Modify ▸ Smooth works on the result if a curve is wanted. | By design |
| 162 | **A wand region has no feather.** Photoshop grows and softens a selection; here the cut edge is a vector boundary, antialiased by the renderer, which is most of what feathering was for. What is missing is deliberately blurring the join between a cut-out and what is behind it. | Follow-up |
| 163 | **The Lasso has no polygon mode.** Animate's lasso has a straight-segment mode on a modifier; here every gesture is freehand. The region is a polygon either way — what is missing is the click-click-click way of drawing one. | Follow-up |
| 164 | **A soft brush cannot paint across strokes.** Each stroke is its own bitmap, sized to itself, so painting over an earlier stroke lays a second bitmap on top rather than adding to the first. Photoshop's model — one canvas per layer that every stroke paints into — would cost a full-canvas copy on every pointer move, because the document is copy-on-write and undo holds the state before. What is lost is smudging and erasing *within* painted pixels; the Eraser still cuts the shape, as it does for any artwork. | By design |
| 165 | **A soft brush paints at the document's own pixel scale**, one painted pixel to one document unit, because that is the resolution the film exports at. Zoomed to 800% the paint is visibly pixelated, where a vector stroke would still be smooth. Painting finer would need a resolution setting and would make every stroke four or sixteen times the memory. | By design |
| 166 | **A long soft stroke is rebuilt from scratch on every pointer move** to preview it: 7.8 ms for a sweep the full width of a 1080p stage, against under 1 ms for an ordinary stroke. Within a frame, and the worst case that exists, but an incremental buffer that only stamps the new segment would make it constant. | Follow-up |
| 167 | **A painted bitmap re-uploads to the GPU on every change.** The whole picture, not the part that changed — which is right for a stroke that arrives complete, and wasteful if painting ever becomes incremental (item 164). Vello has no partial image update, so a dirty-rect upload would mean holding the picture in tiles. | Follow-up |
| 168 | **Layer transparency fades the working view only.** Animate's does the same, and for the same reason: it is a thing you do to see what you are drawing, not a property of the film. A layer meant to be genuinely see-through in the finished picture wants an alpha on its artwork instead. | By design |
| 169 | **A dock column has one scrollbar, so a long panel scrolls the whole column.** The Library is the exception, at a fixed three hundred points, because a big library would otherwise carry everything below it out of reach. The general answer - each panel with its own bounded height, dragged by a splitter - is what Animate has and is a layout engine in its own right. | Follow-up |
| 170 | **Rolled-up panels are remembered in the workspace, not per document.** A preference belonging to the person, like the theme and the dock widths, and it must not travel inside a `.buzz` file. | By design |
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
cargo run --release -p buzz-app -- file.buzz # run, opening a document
cargo run --release -p buzz-app -- art.fla   # run, importing a foreign file
# Run a script over the document at startup, as Animate's command line does.
# The Actions panel opens with the script still in it.
cargo run --release -p buzz-app -- file.buzz --script grid.js
cargo test --workspace                       # 1 084 tests
cargo clippy --workspace --all-targets       # lint
cargo test -p buzz-app --test headless_zoom --release -- --nocapture

# Write a document exercising symbols, folders, instances and all four tween
# span styles, for looking at by hand. Prints the path it wrote.
cargo test -p buzz-doc --test make_fixture -- --ignored --nocapture

# Prove build-up paint and layer depth on the GPU, by reading pixels back.
cargo test -p buzz-app --test headless_build_up -- --nocapture

# Prove an export really holds what the screen shows, pixel by pixel.
cargo test -p buzz-export --test headless_export -- --nocapture

# Prove the lights on the GPU: shading follows the sun, shadows swing with it.
cargo test -p buzz-export --test headless_lighting -- --nocapture

# Prove layer parenting on the GPU: move the body, the head goes with it.
cargo test -p buzz-export --test headless_parenting -- --nocapture

# Prove the filters on the GPU: shadows, glows, blur, blend modes.
cargo test -p buzz-export --test headless_filters -- --nocapture

# Prove the spatial camera on the GPU: tilt turns a rectangle into a trapezoid.
cargo test -p buzz-export --test headless_camera_3d -- --nocapture

# Prove 3D rotation: a tree of turned cards changes shape as the camera moves.
cargo test -p buzz-export --test headless_object_3d -- --nocapture

# Phase 7's exit test: rig an arm, key two poses, tween, save, reopen, export.
cargo test -p buzz-app --test rig_exit_test -- --nocapture

# Sound end to end: import, cue from inside nested symbols, lip sync, reopen.
cargo test -p buzz-app --test audio_lipsync_e2e -- --nocapture

# A document with dialogue, a nested character and a locked mask, to look at.
cargo test -p buzz-app --test make_sound_fixture -- --ignored --nocapture

# Play a tone through the real audio device, to check this machine's output.
cargo test -p buzz-audio --lib device -- --ignored --nocapture

# A five-layer document arranged in depth, with a camera pan to scrub.
cargo test -p buzz-doc --test make_fixture write_depth -- --ignored --nocapture

# Write one .fla, .swf and .pdf fixture, then open one of them.
cargo test -p buzz-app --test make_import_fixtures -- --ignored --nocapture
```

---

## 10. Decisions already taken

Recorded so the same ground is not re-argued. Phase 3 was chosen over closing
the Phase 2 gaps, because a timeline is what makes this an animation tool
rather than a drawing tool. Phase 4 then followed Phase 3 rather than the
importers, because `.fla` and `.swf` need symbols and tweens to exist as
targets before there is anywhere to put what they read.

---

## 11. Next action

**Finishing Phase 6** is the recommended next step. Stills come out now, and
the off-screen render path every other export needs is built and tested — what
is missing is *movement*: nobody publishes an animation as 500 PNGs.

- **CP-6.2** MP4/MOV via NVENC (`h264_nvenc` / `hevc_nvenc` / `av1_nvenc`)
  through `ffmpeg-sidecar`, feeding it the frames `buzz-export` already
  produces, N in flight
- **CP-6.3** GIF/WebP — palette quantisation, animated WebP
- **CP-6.4** HTML5 Canvas / SVG — scene graph to JS plus a small runtime

Closest to the sound work just landed:

- **A sound Properties panel** (§7 items 38 and 39): choosing which sound goes
  on a keyframe, its volume, and its sync mode. The model carries all of it and
  nothing edits it.
- **Honest Event sync** — a sound effect that carries on past its keyframe
  instead of stopping with the playhead.
- **A resampler** (§7 item 40). Nearest-neighbour is fine for animating to
  dialogue and wrong for a finished mix.

Closest to the rigging work before it:

- **A Bind tool** (§7 item 34) — painting weights by hand. Automatic weights
  are right most of the time and wrong exactly where a character creases.
- **Editing a rig** (§7 item 35): deleting a bone, reparenting one, moving a
  joint without posing it. Building is additive today.
- **F6 past the end of a span** (§7 item 37) — a small, real deviation from
  Animate found while animating a rig, and one that costs an animator a frame
  of artwork every time they meet it.

Closest to the scripting work before it:

- **Frame scripts** (§7 item 30). A script that lives in the panel is a tool; a
  script that lives on a keyframe is what makes a document interactive — and it
  is the thing CP-8.3's AS3 runtime will need somewhere to put.
- **Syntax highlighting in the Actions panel** (§6, CP-8.2). The panel is
  usable and plain; a keyword colour and an error squiggle are what stop it
  feeling like a text box.

Closest to the lighting work just landed:

- **Keyframed lights** (§7 item 47) — a sun that swings across a shot. The
  camera already tweens; the rig would take the same path.
- **Lighting in the scripting API** (§7 item 49), so a rig can be built and
  aimed from a script the way shapes and layers already can.

Closest to the drawing and depth work before it:

- **Merge-shape brush strokes** (§7 item 26). Paint that does not fuse with the
  paint under it is the most Animate-unlike thing about the new brushes. The
  booleans are built; the work is doing them off the interactive thread so the
  fix does not cost the responsiveness the brushes were designed around.
- **3D object rotation** (§7 item 28) — the other half of "3D" in Animate:
  rotating an individual movie clip in space, rather than arranging whole
  layers in it. Needs a real projection in the renderer rather than an affine.
- **Tablet pressure** (§7 item 25). The brush already reads pressure; nothing
  supplies it.

Then: library previews (§7 item 17), the Motion Editor (§7 item 18), text
(§7 item 9), and checking the importers against real Adobe files (§7 item 21).
