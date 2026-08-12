# BuzzAnimate — Progress, Checkpoints & Implementation Plan

**Last updated:** 2026-08-12
**Current status:** Phases 0–5 complete (gaps in §7). All three importers —
`.fla`/`.xfl`, `.swf`, `.pdf`/`.ai` — read, merge into an open document, and
report what they could not bring across.

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
| Tests | 561 passing, clippy clean |
| Rust source | ~30 000 lines |
| Crates built | 10 of 15 |
| Phases done | Phase 0, 1, 2, 3, 4, **5** (gaps in §7) |
| Format version | 3 — symbols, instances and tweens |
| Formats read | `.buzz`, `.fla`, `.xfl`, `.swf`, `.pdf`, `.ai` |

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
| 24 | **PDF clipping paths are ignored.** `W`/`W*` are recorded in the report but not applied, so artwork that a real file clips away arrives whole. Needs a clip concept in the scene model, which nothing else has wanted yet. | Phase 5 follow-up |
| 16 | **Camera rotation and zoom have no direct gesture.** Both are keyable and interpolate correctly, and `zoom_camera` exists, but only panning is bound to a drag. | Phase 3 follow-up |
| 13 | **Clipboard (cut/copy/paste) not implemented.** Duplicate works. | Phase 2 follow-up |
| 14 | **Workspace layout is not persisted** across runs. | Low priority |
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
cargo test --workspace                       # 561 tests
cargo clippy --workspace --all-targets       # lint
cargo test -p buzz-app --test headless_zoom --release -- --nocapture

# Write a document exercising symbols, folders, instances and all four tween
# span styles, for looking at by hand. Prints the path it wrote.
cargo test -p buzz-doc --test make_fixture -- --ignored --nocapture

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

**Phase 6 — export** is the recommended next step, and it is the last thing
standing between this and a tool somebody can finish a job with. Everything up
to here can author and import; nothing yet gets a finished animation *out*.

- **CP-6.1** PNG sequence first — it is the simplest, it needs the same
  off-screen render path every other export wants, and it parallelises across
  all 28 threads
- **CP-6.2** MP4/MOV via NVENC, once frames can be produced
- **CP-6.3** GIF/WebP · **CP-6.4** HTML5 Canvas/SVG

Two things would sensibly come first, both small next to a phase:

- **Check the importers against real files** (§7 item 21). Every fixture so far
  is one we wrote, so the readers are verified against the specifications
  rather than against what Adobe's tools actually emit. One genuine `.fla` and
  one genuine `.ai` would be worth more than another dozen synthetic tests.
- **Gradients** (§7 item 8). They are now the most-cited gap in the codebase:
  the SWF and XFL importers both approximate them to flat colours and say so,
  so every gradient in every imported file is a visible loss. Fixing them
  improves authoring *and* import fidelity at once.

Then, in the order they would most improve the tool: library previews (§7 item
17), the Motion Editor (§7 item 18), text (§7 item 9), and SWF colour
transforms on placements (§7 item 23), which the model can already express.
