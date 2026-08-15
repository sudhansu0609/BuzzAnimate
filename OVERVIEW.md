# BuzzAnimate — Overview: what it has, what it cannot do, what is next

**Written:** 2026-08-15 · **Format version:** 18 · **Tests:** 1 486 passing, clippy clean

One page that consolidates the three questions people actually ask about this
program. The detail behind each answer lives elsewhere and is linked:

| File | What it is |
|---|---|
| [`OVERVIEW.md`](OVERVIEW.md) | **This file.** The consolidated state of the tool. |
| [`PROGRESS.md`](PROGRESS.md) | The record: what was built, what was measured, what broke on the way, and the numbered gap list (§7). |
| [`IMPROVEMENTS.md`](IMPROVEMENTS.md) | The plan: what to build next, in what order, and why — Parts I, II and III. |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | The design: **how** the engine waves are built, the rule that keeps the window responsive, and what each costs. |
| [`README.md`](README.md) | The pitch and how to run it. |

> **Rule this project runs on:** a change that is not written down has not been
> finished. Every `§7-nn` below is a numbered row in `PROGRESS.md`, so anything
> stated here can be checked against the record it was written into.

---

## 1. What it is

GPU-accelerated vector animation — Adobe Animate's workflow rebuilt on a modern
engine. Not a new kind of tool: an Animate user should be productive on day one.
The improvements go **underneath** — unbounded zoom, all cores, GPU rasterisation
— not on top, where the buttons are.

**Stack:** Rust · wgpu · Vello · egui · `f64` geometry throughout via `kurbo`.

| | Adobe Animate 2024 | BuzzAnimate |
|---|---|---|
| Maximum zoom | 2 000% | **no cap** — verified to 2×10¹⁴% |
| CPU | effectively single-threaded | **work-stealing pool, all cores** |
| Rasterisation | CPU | **GPU compute shaders (Vello)** |

**16 crates, ~48 000 lines.**

| Crate | Does |
|---|---|
| `buzz-geom` | `f64` geometry and the rebasing camera behind unbounded zoom |
| `buzz-scene` | Copy-on-write document model, Animate-compatible layers, R-tree index |
| `buzz-doc` | The `.buzz` format, undo history, autosave and crash recovery |
| `buzz-render` | GPU device selection and Vello-backed rendering |
| `buzz-jobs` | Work-stealing job system and utilisation metrics |
| `buzz-ui` | Theme, menus, panels, stage chrome |
| `buzz-app` | The application shell |
| `buzz-rig` | Armatures, IK, skinning, mesh warping |
| `buzz-light` | Sun, sky and lamp; shading and cast shadows |
| `buzz-fx` | Filters and blend modes, as vector geometry |
| `buzz-audio` | Decoding, waveforms, playback, lip-sync analysis |
| `buzz-export` | Finished frames out: PNG images and sequences, MP4/MOV via NVENC |
| `buzz-script` | Sandboxed JavaScript API over the document |
| `buzz-import-xfl` · `-swf` · `-pdf` | The three importers |

---

## 2. What is implemented

### Phases

| Phase | State |
|---|---|
| 0 — Foundations | ✅ complete |
| 1 — Geometry & document core | ✅ complete |
| 2 — Stage, tools & UI shell | ✅ substantially complete |
| 3 — Timeline & frame animation | ✅ complete |
| 4 — Symbols, library & tweens | ✅ complete |
| 5 — Importers | ✅ complete |
| 6 — Export | 🟡 **PNG + MP4/MOV with sound** (CP-6.1, CP-6.2). GIF/WebP and HTML5 not built |
| 7 — Rigging | ✅ complete |
| 8 — Scripting & ActionScript | 🟡 **JS scripting only** (CP-8.1). No AS3 runtime or compiler |

### Drawing

- **23 tools** — Selection, Subselection, Lasso, Magic Wand, Free Transform,
  Gradient Transform, Pen, Text, Line, Rectangle, Oval, PolyStar, Pencil, Brush,
  Bone, Asset Warp, Paint Bucket, Ink Bottle, Eyedropper, Eraser, Camera, Hand,
  Zoom — on Animate's own letters. **22 of them work; Text is inert** (§7-9), and
  a test asserts it is the only one.
- **4 brush kinds** — Fluid, Pattern, Art, Soft (raster) — with **7 pattern
  shapes** including one made from a selection.
- Gradients: linear and radial, on fills and strokes, with a working Gradient
  Transform tool.
- Shape recognition (circles, ovals, squares, rectangles at any angle), smoothing,
  straightening, boolean operations, path offsetting.
- Bitmaps as a paint; Lasso that *cuts* the artwork it crosses; Magic Wand.

### Document & timeline

- **7 layer kinds** — Normal, Folder, Mask, **Inverse Mask**, Masked, Guide, Guided.
- Layer parenting, per-layer depth, colour, transparency, outline view, lock, hide.
- **Cut, Copy and Paste** — artwork carries the symbols it needs and moves between
  documents. **Align, Distribute and Match Size** in `Modify ▸ Align`, with or without
  the stage as the frame. **Arrow-key nudge**, one unit and eight with Shift.
- Timeline with Animate's frame conventions, onion skin, Edit Multiple Frames,
  Auto Keyframe, a looping section, two zooms, and layer switches on the rows.
- **Tweens** — classic, motion and shape, with easing in the model.
- Symbols (Graphic, MovieClip, Button), nested timelines, a Library with folders,
  search, usage counts and **a thumbnail of every symbol**.
- Camera with keyframes, spatial 3D rotation, layer depth and a real projection.
- Undo/redo by snapshot; autosave; crash recovery offered on launch.

### Look

- **5 filters** — Blur, Drop Shadow, Glow, Bevel, Colour Adjust.
- **10 blend modes** — Normal, Layer, Darken, Multiply, Lighten, Screen, Overlay,
  Hard Light, Add, Difference.
- **Lighting** — sun, sky and lamp, with shading, highlights and cast shadows.
  A deliberate addition; Animate has no lights (§7-44).
- Named swatches in folders; dark and light themes.

### Sound

- Decodes `.wav` `.mp3` `.ogg` `.flac` `.m4a` `.aac`.
- A soundtrack is audible from inside any symbol at any depth, draws its waveform
  in the timeline, and drives **automatic lip sync**.

### Rigging

- Bone tool; armatures over shapes and symbol chains.
- **FABRIK IK** with angle limits and pin constraints.
- Puppet warp via Moving Least Squares; vertex weight binding; poses that tween.
- Budget: 50 six-bone rigs solved in parallel, well inside one frame.

### In and out

| Direction | Formats |
|---|---|
| **Reads** | `.buzz` · `.fla` · `.xfl` · `.swf` · `.pdf` · `.ai` |
| **Writes** | `.buzz` · PNG image · PNG sequence · **MP4 / MOV, with the soundtrack muxed in** |

Video encodes on the GPU through NVENC (`h264_nvenc` / `hevc_nvenc` / `av1_nvenc`)
with software fallbacks, driving the ffmpeg already on the machine — frames are piped
as they render, so a 500-frame export never lands 4 GB of PNG on the disk.

All three importers merge into an **already-open** document, remapping every id,
and report what they could not bring across.

### Scripting

Sandboxed JavaScript through Animate's own `fl` / `document` API — rectangles,
ovals, layers, frames, selection, the library and document properties.

---

## 3. Restrictions

Four kinds, and the difference matters: some of these will never change, some are
waiting on other people, some are simply not built yet, and some are choices.

### 3.1 Hard limits — architectural, not going away soon

| Limit | Why | Ref |
|---|---|---|
| **egui pinned at 0.35** | 0.36 needs wgpu 30; vello 0.9 needs wgpu 29. Two wgpu majors cannot share a device. **Blocked on vello.** | §7-2 |
| **`f64` precision floor** | Sub-pixel accuracy to ~1e12%, linear decay after. By design. | §7-4 |
| **Filters and lighting are geometry, not raster passes** | Vello offers no shader hook, so a shaded side is a boolean crescent and a blur is built from bands. Keeps everything editable; will not match a true raster blur. Holds for *per-shape* effects only — a **full-frame** pass is possible at the seam where Vello's output is blitted, which is what `ARCHITECTURE.md` Wave 6 builds on. | §7-46, §7-52 |
| **No tablet pressure** | winit 0.30 supplies none on Windows. The brush reads pressure and the setting exists; every sample arrives at 1.0. | §7-25 |
| **Legacy binary `.fla`** (CS4 and older, OLE2) | Out of scope. | §7-5 |
| **No `.fla` write-back** | Import only. Not planned for v1. | §7-6 |
| **egui is immediate-mode** | Chosen to reach a working app fast; acknowledged as not ideal long-term for a pro tool. | §7-3 |
| **Windows-only launcher** | `BuzzAnimate.bat` is a batch file; elsewhere it is `cargo run --release -p buzz-app`. | §7-106 |

### 3.2 Not built yet — the significant absences

| Missing | Consequence | Ref |
|---|---|---|
| **Text tool** | No titles or credits without drawing them by hand or importing them. Needs font loading, shaping and an editing caret. | §7-9 |
| **GIF/WebP and HTML5 export** | PNG, PNG sequence and MP4/MOV are the outputs; a GIF for a preview or a self-playing HTML5 build are not there. | CP-6.3–6.4 |
| **Export blocks the document** | One export at a time, and its progress dialog belongs to the document — opening a new file mid-export orphans it. Quitting kills an export in flight. | Wave 5 |
| **Thumbnails in the Assets panel** | The Library draws a picture of every symbol; an asset is a file on disk, so a picture of one needs a background read. | §7-81 |
| **Motion Editor, motion paths, shape hints** | Easing exists in the model and interpolates; nothing edits the curve, and a tween cannot follow a drawn path. | §7-18 |
| **Bitmap import in all three readers** | XFL, SWF and PDF count and report their bitmaps as skipped. The pipeline they need now exists. | §7-158, §7-116 |
| **Multiple scenes** | One scene per document. | §7-12 |
| **AS3 runtime and compiler** | JavaScript scripting works; ActionScript does not exist. | CP-8.3–8.4 |
| **Bézier pen, Bind tool, bone delete/reparent** | The Pen draws segments; skin weights cannot be painted; rig building is additive-only. | §7-11, §7-34, §7-35 |
| **Depth of field** | Layers off the focal plane stay perfectly sharp. | §7-29 |

### 3.3 Deliberate deviations from Animate

Each is off by default or additive, so a document that does not use it behaves
exactly as Animate would.

| Deviation | Ref |
|---|---|
| **Lighting** — Animate has no lights at all | §7-44 |
| **Build-up paint** — Animate's shapes always composite source-over | §7-27 |
| **Inverse mask** — a `.fla` cannot express one, so a document using it will not round-trip | §7-119 |
| **The looping section** lives in the document and the exporter repeats it; Animate's loop is a transport setting | §7-66 |
| **Auto Keyframe** — Animate has no such mode | §7-70 |
| **Named swatches in folders** — Animate's panel is a flat grid of unnamed chips | §7-76 |
| **The Assets panel** — Animate's ships curated content and syncs with Creative Cloud | §7-80 |
| **Panels move by menu, not by dragging** — grouping into tabs now works (§7-57 resolved); dragging a panel into a dock does not | §7-56 |
| **One workspace**, plus Reset — Animate saves named workspaces | §7-58 |
| **Two themes, not a theme editor** | §7-89 |

### 3.4 The long tail

`PROGRESS.md` §7 carries **155 numbered rows, 140 of them still open.** They are
the honest record of every place this program does something a reasonable person
would not expect, from *"a soft brush cannot paint across strokes"* (§7-164) to
*"a recovered document does not remember what it was"* (§7-102). Most are small,
all are written down, and none of them is a surprise waiting to be discovered.

---

## 4. Suggested improvements

The full argument, with sizing and what already exists to build on, is in
[`IMPROVEMENTS.md`](IMPROVEMENTS.md). In brief:

> **The diagnosis: the tool is built for drawing, and the work being done is
> assembly** — placing characters you already own, into stages you have already
> built, in poses you have already worked out. Almost none of what makes that slow
> is hard. It is missing.

### Wave 1 — stop retyping work already done

| | Size | |
|---|---|---|
| **The clipboard** | M | `Scene::extract` and `Scene::merge` both already exist and are tested. Mostly wiring. |
| **Thumbnails** | L | Off-thread raster into a cache keyed by symbol and revision. Turns choosing-by-name into choosing-by-sight. |
| **Drag from panel, drop on stage** | S | Removes two steps from every placement. |

### Wave 2 — poses become things you own

**Pose Library** (M) stored on the `Symbol` so it travels with the character ·
**Mirror a pose** (S) · **Pose-to-pose keying** (S), which turns a pose library
into a way of *animating* · **Bone delete and reparent** (M).

### Wave 3 — set the stage once

**Scene templates** (M) · **Align and Distribute** (S) · **Arrow-key nudge** (S) ·
**Live transform preview** (S–M).

### And the beautiful half

**Motion Editor and motion paths** (L, §7-18) — the single biggest available
change to how motion *feels* · **Keyframed lights** (M, §7-47) · **Depth of
field** (M, §7-29) · **The Text tool** (L, §7-9).

### Recommended order

```
Clipboard  →  Thumbnails  →  Drag-to-place
```

All three are absences rather than hard problems; all three are repaid every time
a character is placed; and thumbnails are what makes the Pose Library worth
building at all — a pose you cannot see is a pose you will not reuse.

### Beyond that: Parts II and III

Part I above makes the tool quicker. **Part II changes what it is** — one program for
vector *and* raster drawing, exporting in the background while you start the next shot,
lit and composited like a 3D package, on an engine with one rule: *the window never
stops responding.* Designs in [`ARCHITECTURE.md`](ARCHITECTURE.md).

| Wave | | Size |
|---|---|---|
| 4 | Foundations — the task registry; everything long comes off the UI thread | M |
| 5 | Background export — queue, presets, Tasks panel, GIF/WebP | M |
| 6 | Compositor — bloom, grain, vignette, grade; cheap depth of field | M |
| 7 | **Raster layers** — paint properly, beside the vectors | **L** |
| 8 | Asset pipeline — watched folders | S |
| 9 | 2.5D — keyframed lights, depth sorting, real depth of field | M+S+M |
| 10 | The film — `.buzzproj`, many shots into one movie | M |
| 10b | **Camera angles** — stage once, shoot from anywhere | S–M |

**Part III** is the delight half: motion trails and arcs, audio scrubbing, a video
reference layer for rotoscoping; the **procedural modifier stack** (springs on bones =
automatic follow-through); stabiliser, symmetry and perspective guides; true motion
blur and alpha video at export; a command palette.

---

## 5. Numbers

| Measure | Value |
|---|---|
| Zoom verified | 2×10¹⁴% (Animate: 2 000%) |
| Render quality across 13 decades | constant ~4.01% ink |
| GPU frame time at 1e12% | 0.9 ms |
| Threads in use | 20 interactive + 6 background |
| Tests | 1 486 passing, clippy clean |
| Rust source | ~48 000 lines across 16 crates |
| Format version | 18 |
| IK budget | 50 six-bone rigs in parallel, inside one frame |
| First frame of a heavy scene | 305 ms, then cached (§7-155) |
| Open items in §7 | 140 of 155 |

---

## 6. Keeping this file true

`OVERVIEW.md` is a **summary of the other files** and holds no facts of its own.
When something ships:

1. `PROGRESS.md` §4 gets a section saying what was built and **why it was built
   that way**.
2. Its `§7-nn` row is struck through and marked resolved.
3. Its row in `IMPROVEMENTS.md` is struck through with a pointer to that section,
   and — for a Part II or III wave — the design in `ARCHITECTURE.md` is marked
   shipped, with any difference between design and build recorded in §4.
4. **Then** the relevant table here is moved — out of §3 and into §2, or out of §4.

If this file and `PROGRESS.md` ever disagree, `PROGRESS.md` is right.
