# BuzzAnimate — Improvements

**Written:** 2026-08-15
**Companion to:** [`PROGRESS.md`](PROGRESS.md) — every `§7-nn` below is a numbered
row in that file's gap list, so each item can be looked up against the record it
was written into. **Designs for Parts II and III are in**
[`ARCHITECTURE.md`](ARCHITECTURE.md).
**Published as:** <https://claude.ai/code/artifact/e5f3d659-e62d-4123-8818-e0ced195ae0c>

| Part | Waves | What it is |
|---|---|---|
| **I — closing the assembly gap** | 1–3 | Stop retyping work already done. Placing a character and posing it. |
| **II — the engine waves** | 4–10b | All-in-one: raster beside vector, background export, compositor, 2.5D, films — on an engine that never hangs. |
| **III — the delight waves** | 11–15 | Beyond parity: the things that make animating *good* rather than merely possible. |

---

# Part I — closing the assembly gap

---

## 1. The diagnosis

> **The tool is built for drawing. The work being done is assembly.**

Every drawing and animating primitive is in place: brushes, tweens, rigs with
FABRIK IK, lighting, sound, lip sync, three importers, an exporter. What is thin
is the layer *above* them — the part that treats a character as **a thing you own
and reuse** rather than something you make once.

That shows up as a specific, repeated cost, and it is the one the studio actually
reported: *setting up a stage and putting different characters and poses into it
takes time and effort.*

The important finding is that **almost none of it is hard**. It is missing.

---

## 2. Where the time goes

What happens when you put **one** character into **one** scene, today:

| # | Step | How it works now | What it costs |
|---|---|---|---|
| 1 | Find the character | A text list — name, kind, use count | You open symbols to see what they are |
| 2 | Get it on stage | Select it, press **Place** | Lands where it was drawn, not where you want it |
| 3 | Position it | Drag; snapping does work | No nudge, no align, no preview while rotating |
| 4 | Pose it | Drag the IK handles | From scratch — every pose, every time |
| 5 | Add the next one | Repeat all of the above | Copy and paste do nothing at all |
| 6 | Set the stage | Background, camera, lights, by hand | No template — every film starts empty |

---

## 3. What was verified, and where

Read out of the source rather than assumed, so the plan rests on the code.

| Where | Finding |
|---|---|
| `crates/buzz-app/src/editor.rs:1664` | **Copy and Paste are stubs.** They set a status line reading *“Clipboard arrives with the Phase 2 follow-up”*. `Cut` simply calls `delete_selection`. |
| §7-17, §7-81 | **No thumbnails anywhere.** The Library and the Assets panel both identify a character by its name. |
| §7-82 | **A placed asset keeps its old coordinates** rather than landing under the pointer. |
| `crates/buzz-ui/src/rig_panel.rs` | **There is no pose library.** `RigResponse` carries exactly two pose commands — `reset_pose` and `set_rest_pose`. A pose only ever exists as a keyframe. |
| `crates/buzz-ui/src/command.rs` | **No Align or Distribute.** Zero matches in the whole command set; Animate's `Ctrl+K` has no equivalent here. |
| `crates/buzz-scene/src/lib.rs:1171`, `merge.rs:191` | **The hard halves already exist.** `Scene::extract(frame, ids)` pulls objects *and the symbols they need*; `Scene::merge(other, target)` takes a scene in and remaps every id. Both are tested. |
| `crates/buzz-rig/src/lib.rs:308–331` | **So does the pose maths.** `pose()`, `set_pose()` clamping to joint limits, and `tween_pose()` turning each joint the shortest way round. |
| `crates/buzz-ui/src/view.rs:30` | **Snapping is not a gap.** Guides, grid, objects and pixels all exist; objects and guides are on by default. |

---

## 4. The plan

Three waves, in order — each makes the next worth having. **Sizes are relative to
what already exists in the codebase, not absolute effort.**

### Wave 1 — Stop retyping work you have already done

The assembly primitives. None is a hard problem; all three are simply absent, and
each is repaid on every character placed.

| Item | Size | What it is | What is already there |
|---|---|---|---|
| ~~**The clipboard**~~ ✅ | M | ~~Cut/Copy/Paste for artwork, within a document and across two of them.~~ **Shipped** — see `PROGRESS.md` §4 *Wave 1.1 — The clipboard*. Closed §7-13. | |
| **Thumbnails** | L | Rasterise each symbol off-thread into a cache keyed by symbol **and revision**. Turns choosing-by-name into choosing-by-sight. | A Vello renderer and a job system. New: the cache, and knowing when to invalidate it. Closes §7-17 and §7-81. |
| **Drag from panel, drop on stage** | S | It lands where you let go. Removes two steps — press Place, then hunt for it and drag it — from every placement. | Closes §7-82. |

> **Wave 1 alone changes what the tool *is* for assembly work.**

### Wave 2 — Poses become things you own

The direct answer to *“putting different poses takes time”*. A pose is currently a
fact about one keyframe; it should be a named thing that belongs to the character
and travels with it.

| Item | Size | What it is | What is already there |
|---|---|---|---|
| **The Pose Library** | M | Name the pose on screen; store it **on the `Symbol`**, so it travels into the Assets library and into every other document that character appears in. Apply with one click, with a thumbnail from Wave 1. | `pose()` and `set_pose()`. New: a named list on `Symbol` (which today holds only id, name, kind, folder, layers, registration) and a format-version bump. |
| **Mirror a pose** | S | The same pose, other side: reflect the angles about the rig's axis and swap left/right bone pairs. Halves the work of building a pose set. | |
| **Pose-to-pose keying** | S | Pose A on frame 1, pose B on frame 12, and the tween between them is the whole animation. **This is the actual speed-up** — it turns a pose library from a posing aid into a way of *animating*. | `tween_pose`, which already turns each joint the shortest way round. |
| **Fix a rig without rebuilding it** | M | Delete and reparent bones. Rig building is additive-only today, so one wrong bone means starting the skeleton again — which is why nobody rigs the second character. | Closes §7-35. |

### Wave 3 — Set the stage once

The remaining half of the complaint, plus the small daily frictions that are
individually trivial and collectively constant.

| Item | Size | What it is | What is already there |
|---|---|---|---|
| **Scene templates** | M | Save a whole stage — background, characters, camera, lights, size, frame rate — as a named starting point, and begin a film from it. An asset today carries its objects and sounds but deliberately leaves the stage behind. | Extract and merge again. Extends §7-84. |
| ~~**Align and Distribute**~~ ✅ | S | ~~Animate's `Ctrl+K`.~~ **Shipped** as `Modify ▸ Align` — six alignments, the same six to the stage, two kinds of distribute and Match Size. See `PROGRESS.md` §4 *Wave 3.2*. | |
| ~~**Arrow-key nudge**~~ ✅ | S | ~~One unit per press, eight with Shift.~~ **Shipped** — see `PROGRESS.md` §4 *Wave 3.3 — The arrow keys*. Closed §7-72. | |
| **Live transform preview** | S–M | Artwork that redraws *while* you rotate it rather than on release. The maths is identical either way; only the feedback changes. | Closes §7-86. |

---

## 5. Start here

```
Clipboard  →  Thumbnails  →  Drag-to-place
```

In that order, and the order is the argument:

- All three are **absences rather than hard problems**.
- All three are **repaid every time a character is placed** — the most repeated
  action in the workflow that was reported as slow.
- **Thumbnails are what makes the Pose Library worth building at all.** A pose you
  cannot see is a pose you will not reuse, so building Wave 2 first would produce
  a feature that looks finished and goes unused.

---

## 6. The beautiful half

Secondary, because speed is the stated aim — but this is where the work stops
looking competent and starts looking good.

| Item | Size | Why it matters | Gap |
|---|---|---|---|
| **Motion Editor and motion paths** | L | Easing already exists in the model and interpolates correctly; nothing in the interface edits the curve, and a tween cannot follow a drawn path. **The single biggest available change to how motion feels** — the difference between things moving and things having weight. | §7-18 |
| **Keyframed lights** | M | The rig belongs to the document rather than the timeline, so a sun cannot swing through a shot the way the camera can. Putting it on the tween path it already shares with the camera is most of the work. | §7-47 |
| **Depth of field** | M | Layer depth exists and the camera has a focal distance; layers off that plane are still perfectly sharp however far away they sit. Blur with distance and a flat stack starts reading as space. | §7-29 |
| **The Text tool** | L | Font loading, shaping and an editing caret — a subsystem of its own, and the reason titles and credits have to be drawn by hand or brought in from elsewhere. | §7-9 |

---

## 7. Deliberately not now

Named so the omissions are decisions rather than oversights.

| Item | Why not | Gap |
|---|---|---|
| **Multiple scenes** | A scene template gets most of the benefit for a fraction of the work, and does not touch the document model. | §7-12 |
| **The Bind tool** | Painting skin weights by hand matters when a rig is *wrong*. It is not on the path between an animator and a populated stage. | §7-34 |
| **Dragging panels to dock them** | The menu does the job, panel groups landed this week, and this is polish rather than speed. | §7-56 |
| **Bitmap import in the three readers** | Real, and a different complaint — it blocks bringing work *in*, not building work here. | §7-158 |

---

## 8. How this file is kept

The same rule `PROGRESS.md` uses: **an item that is not written down here has not
been finished.** When one of the above is built, it gets a section in
`PROGRESS.md` §4 saying what was built and *why it was built that way*, its
`§7-nn` row is marked resolved, and its row here is struck through with a pointer
to that section.

---

# Part II — the engine waves

> **The ambition changes here.** Part I makes an animation tool quicker to work in.
> Part II makes it a different kind of program: **one tool for vector *and* raster
> drawing, that exports in the background while you start the next shot, lights and
> composites like a 3D package, and never stops responding.**

The design — types, crate placement, format bumps, testing and the risk register — is
in [`ARCHITECTURE.md`](ARCHITECTURE.md). This is the summary.

### The rule the whole of Part II is arranged around

> **The window must never stop responding.** Not for a script, not for an export, not
> for a heavy first frame, not for a file dialog.

Six things break that rule today, and all six are named with their file and line in
`ARCHITECTURE.md` §0. Wave 4 exists to close them, and it gates everything else.

### The waves

| # | Wave | Size | What it gets you | Depends on |
|---|---|---|---|---|
| **4** | **Foundations — the task registry** | M | Long work gets somewhere to live: progress, cancel, and a guarantee it survives closing the document. Scripts, imports, file dialogs and the 305 ms first frame all come off the UI thread. | — |
| **5** | **Background export: queue, presets, Tasks panel** | M | Start the next shot while one renders. A queue instead of one slot, named presets, a global panel showing what the program is doing, and a prompt before quitting throws an export away. Adds **GIF/WebP**. | 4 |
| **6** | **Compositor — bloom, grain, vignette, grade** | M | The frame stops looking like flat vector output. Same code on the stage and in the export, so the preview *is* the result. Cheap depth-of-field arrives here too. | — |
| **7** | **Raster layers** | **L** | **Paint properly.** One canvas per layer, strokes that merge, a working eraser, raster filters, tablet pressure. Krita-lite beside the vectors. | 4 (lightly) |
| **8** | **Asset pipeline — watched folders** | S | Drop a file in the assets folder and it is there, with a thumbnail. No refresh button. | 4, thumbnails |
| **9** | **2.5D — keyframed lights, depth sorting, real DOF** | M+S+M | A sun that swings through a shot. Cards that cross in space drawing in the right order. Focus that falls off with distance. | 4, 6 |
| **10** | **The film — `.buzzproj`** | M | Many shots, one movie. Shots stay separate files; the export queue renders them and stitches them. | 5 |
| **10b** | **Camera angles — stage once, shoot from anywhere** | S–M | **Stop re-staging a scene for a different angle.** Save named camera angles, cut between them on one timeline, or render the same staged scene as N shots at N angles. | 9b |

### Wave 10b, because it answers a question that was asked twice

Setting up a scene takes time; setting it up *again* for a different angle takes it
again. It should not, and it does not need to — **an angle is a camera state, not a new
scene.** The camera already carries centre, zoom, rotation, pitch, yaw and focal
distance, and already projects the stage in real perspective with per-layer parallax.
Naming that state is one new field.

From it: an Angles panel to save and jump between "Wide", "Close on Ana", "Reverse";
**"Cut to angle at playhead"**, so a whole multi-angle sequence lives on **one timeline
of one staged scene**; and `Shot.angle` in a project file, so the same staged document
becomes several shots in the finished film with no re-staging at all.

The honest limit, stated so it is not discovered later: **flat art seen edge-on is
flat.** Moderate pitch and yaw is the believable envelope; a true reverse on a
character needs a second drawing, which is what a turnaround in the pose library
(Wave 2) is for. Depth sorting (9b) and parallax are what make a re-angle read as a
genuinely new view.

### What Part II closes

§7-25 (tablet pressure) · §7-29 (depth of field) · §7-32 (scripts on the UI thread) ·
§7-47 (keyframed lights) · §7-83 (assets folder not watched) · §7-154 (lighting
single-threaded) · §7-155 (305 ms first frame) · §7-164, §7-165, §7-166, §7-167 (the
four raster limits) · CP-6.3 (GIF/WebP). §7-60 and §7-65 are addressed by an opt-in
rather than closed outright.

Format versions: **19** (compositor) · **20** (raster layers) · **21** (light tracks,
depth sort, aperture, named angles).

---

# Part III — the delight waves

Part II makes the program fast, safe and capable. Part III is what makes it *good*.
Each item is grounded in a subsystem that already exists — none is a moonshot. Designs
in [`ARCHITECTURE.md`](ARCHITECTURE.md) Part III.

| # | Wave | Size | The idea |
|---|---|---|---|
| **11** | **Animation feel** | M | **Motion trails and arcs** on the stage — bunched ticks are slow, a lumpy arc is why the motion looks wrong. **Audio scrubbing** while dragging the playhead. **Video reference layer** — rotoscoping, using the ffmpeg already depended on. **Frame labels and beat markers** detected from the soundtrack. |
| **12** | **Procedural motion — the modifier stack** | **L** | **The differentiator.** `Wiggle`, `Spring`, `LookAt`, `AutoSquashStretch` on objects and bones, evaluated at draw time and deterministic in `(object, frame)`. **Automatic follow-through and overlap** for hair, cloth and tails: the most labour-intensive thing an animator does by hand, and the first thing cut when time is short. |
| **13** | **Drawing delight** | M | Pull-string **stabiliser**; **symmetry** drawing (mirror X/Y/radial); **perspective guides**; a **gap-aware paint bucket** (Animate's Gap Size — the reason its bucket feels forgiving); gradient maps and paper-texture fills. |
| **14** | **Pro output** | M | **True motion blur at export** — sub-frame accumulation on the GPU, which nothing in the Animate world ships. **Alpha video** (ProRes 4444 / VP9+alpha) so the work composites elsewhere. **Render region.** Posterise, halftone and hatching in the compositor. |
| **15** | **Command and control** | S | **Command palette** on `Ctrl+K` — the command catalogue, its labels and its shortcuts are already data, so this is a search box over a list that exists. **Shortcut editor.** **Saved commands** for Actions scripts. **Named version snapshots** through the autosave machinery. |

**Parked, and named so it is a decision:** ML-assisted inbetweening and colourisation
(research-grade, heavy dependencies) · HTML5 runtime export (CP-6.4, already on the
roadmap) · multiple scenes in one file (superseded by `.buzzproj` unless a reason
appears it cannot serve) · collaborative review.

### Suggested order after Part II

```
15 (small, immediate)  →  11  →  13  →  14  →  12
```

The modifier stack goes last: it is the biggest, and it is much better once keyframed
lights and a motion editor already exist.
