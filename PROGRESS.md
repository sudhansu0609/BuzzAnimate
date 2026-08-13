# BuzzAnimate — Progress, Checkpoints & Implementation Plan

**Last updated:** 2026-08-13
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
| Tests | 1 084 passing, clippy clean |
| Rust source | ~48 000 lines |
| Crates built | 16 of 17 |
| Phases done | 0, 1, 2, 3, 4, **5**, **7** (gaps in §7), plus CP-6.1 and CP-8.1 |
| Format version | 11 — adds 3D rotation on objects |
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

| # | Item | Status |
|---|---|---|
| 1 | ~~**Oversized paths culled, not clipped.**~~ | ✅ **Resolved in CP-1.1** by `RenderClip` |
| 8 | **Gradients not implemented.** Fills are solid colours only; the Gradient Transform tool is inert. Touches five crates (paint model, renderer brush, serialisation, editor UI). | Phase 2 follow-up |
| 9 | **Text tool not implemented.** Needs font loading, shaping and a text-editing caret — a subsystem in its own right. | Phase 2 follow-up |
| 10 | **Lasso tool not implemented.** Freehand selection region. | Phase 2 follow-up |
| 11 | **Pen tool draws line segments, not Bézier curves.** Click-drag handle authoring is not there yet; anchors can be edited afterwards with Subselection. | Phase 2 follow-up |
| 12 | **Multiple Scenes not implemented.** One scene per document. | Deferred |
| 15 | ~~**Tweening not implemented.**~~ | ✅ **Resolved in CP-4.3** — classic, motion and shape tweens interpolate in the render path |
| 17 | **Library has no previews.** Symbols are listed by name, kind and use count; there is no thumbnail. Needs off-thread rasterisation into a cache keyed by symbol and revision. | Phase 4 follow-up |
| 18 | **No Motion Editor, motion paths or shape hints.** Easing exists in the model (strength and cubic Bézier) and interpolates correctly, but nothing in the UI edits a curve, and a motion tween cannot yet follow a drawn path. | Phase 4 follow-up |
| 19 | ~~**Import commands are not wired.**~~ | ✅ **Resolved in CP-5.1b** — `Scene::merge` remaps every id; all three formats are on the File menu |
| 20 | ~~**The XFL importer does not restore folder nesting.**~~ | ✅ **Resolved in CP-5.1c**, along with two fidelity bugs it exposed |
| 21 | **No importer has been checked against a real file from Adobe.** Every fixture is one we wrote, so the importers are verified against the *specifications* and against files whose content we chose — not against what Animate, Illustrator and the Flash compilers actually emit, which is where the awkward cases live. This is the largest single risk in Phase 5. | Needs a licensed Animate/Illustrator and real-world files |
| 22 | **Bitmaps are not imported** by any of the three readers — reported, never read. Needs a media pipeline: decode, store in the `.buzz` container's `media/` directory (reserved since Phase 1), and a bitmap object kind. | Phase 6 |
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
| 55 | **Gradient Glow and Gradient Bevel are not implemented**, because gradients are not (§7 item 8). Both are the plain Glow and Bevel with a ramp instead of a colour, so they arrive with gradients. | Blocked on gradients |
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
- **Gradients** (§7 item 8), still the most-cited gap: both importers
  approximate them to flat colours, and a pattern brush cannot stamp one.
- **Tablet pressure** (§7 item 25). The brush already reads pressure; nothing
  supplies it.

Then: library previews (§7 item 17), the Motion Editor (§7 item 18), text
(§7 item 9), and checking the importers against real Adobe files (§7 item 21).
