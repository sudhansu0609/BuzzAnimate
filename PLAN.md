# Outstanding Items — Execution Plan

**Derived from:** `PROGRESS.md` §7 (the record), `IMPROVEMENTS.md` Parts I–III, `ARCHITECTURE.md`.
If this file and `PROGRESS.md` ever disagree, **`PROGRESS.md` is right.**

State at writing: 138 of 155 §7 rows open per `PROGRESS.md`. **That record is stale.**
Verified against source on this pass — see the audit below; Part II is fully built,
Part III mostly built, and only a short tail remains genuinely open.

---

## -1. Source-verified status (audit of every wave)

**The document format version in code is 41** (`crates/buzz-doc/src/serial.rs:174`),
not the 21 that `IMPROVEMENTS.md` budgets — so many "open" rows below were closed by
work never written into `PROGRESS.md`.

| Item | Status in source | Evidence |
|---|---|---|
| Wave 4 task registry | ✅ built | `buzz-app/src/tasks.rs:216 TaskRegistry`, wired at `app.rs:726`; exports are one thread each (`export_service.rs`) |
| Wave 5 background export, GIF/WebP | ✅ built | `export_service.rs` queue; `buzz-export/src/gif.rs` ("CP-6.3 — animated GIF and WebP"), presets in `preset.rs` |
| Wave 6 compositor (bloom/grade/vignette/grain) + cheap DOF | ✅ built, plus posterise/halftone/hatching flags | `buzz-render/src/compositor.rs:10`; aperture blur at `document.rs:1927` ("a document that sets no aperture is untouched") |
| Wave 7 raster layers | ✅ built | `buzz-scene/src/raster.rs` — `Canvas`, `SoftBrush`, `MergedPaint::merge_over`, per-stroke canvases |
| Wave 9 keyframed lights / depth sort / real DOF | ✅ built | `LightTrack` in scene (`lib.rs:67`); `LayerStack::depth_paint_order` + opt-in `sort_by_depth` (`layer.rs:736`); aperture blur above |
| Wave 10b camera angles | ✅ built | `NamedAngle`, `CameraTrack.angles`, save/lookup at `camera_track.rs:283,957`; `Shot.angle` in project (`project.rs:57`) |
| Wave 10 `.buzzproj` film | ✅ built | `buzz-doc/src/project.rs` — manifest, `PROJECT_EXTENSION = "buzzproj"`, shot list |
| Wave 12 modifier stack (Part III) | ✅ built | `buzz-scene/src/modifier.rs:46` — `AutoSquashStretch` et al., serialised (`serial.rs:1453`), exposed to UI (`filter_panel.rs`) and JS (`buzz-script/film.rs`) |
| Wave 14 motion blur + alpha video | ✅ built | Sub-frame shutter accumulation in export (`buzz-export/src/lib.rs:294 render_blurred`, `shutter_offsets`); **ProRes 4444 (alpha)** codec in `video.rs:53` |
| Wave 15 command palette / shortcuts / saved commands | ✅ built | `buzz-ui/src/command_palette.rs` (Ctrl+K), `shortcut_editor.rs`; `SavedCommand`/`CommandLibrary` in `buzz-doc/src/commands.rs` |
| Wave 11 audio scrub + video reference layer | ✅ built | `editor.scrub_audio`, `import_video_reference` (`app.rs:3863,5123`) with a real test; motion-trail arcs **not found** — likely still open |
| Wave 13 stabiliser | ✅ built | Pull-string stabiliser in `buzz-geom/src/brush.rs:375`, used by pencil+brush (`tools.rs:1320`); symmetry drawing and perspective guides **not found** |

### §7 rows the code has closed but the record still lists open

| Row | Code says | Evidence |
|---|---|---|
| §7-38 sound Properties panel | ✅ built | `buzz-ui/src/sound_panel.rs:58` |
| §7-39 sync modes actually differ | ✅ built | `player.rs`: Event voices render on their own clock even when stopped; Start declines to stack — distinct behaviour, tested |
| §7-40 resampling | 🟡 improved | `player.rs:928` test "resampling interpolates rather than repeating the nearest sample" — linear now, not sinc |
| §7-30 saved scripts | ✅ built (saved commands) | `buzz-doc/src/commands.rs`; frame-script execution hook unverified |

### Genuinely still open in source (checked, absent)

- **§7-158 bitmap import in the three readers** — SWF still substitutes a flat colour and flags it (`buzz-import-swf/src/shape.rs:339` "no reader here imports bitmaps")
- **§7-24 PDF clipping paths** — `W`/`W*` still only reported, not applied (`buzz-import-pdf/src/lib.rs:430`)
- **Bézier pen drag-handle authoring** (§7-11), **shape hints** (no such type in scene; `tween.rs:23` comment confirms absence)
- **Bind tool / joint speed** — no weight-painting or per-joint damping anywhere in `buzz-rig`
- **Gimbal drag for 3D rotation**, named workspaces, gradient glow/bevel filters, the four missing blend modes (Subtract/Invert/Alpha/Erase), `.clr/.act/.ase` palette I/O — no traces found

**Conclusion:** Phases A–C of this plan are done; Phase D is ~80% done. The live
outstanding list collapses to: importer fidelity (bitmaps, PDF clips, real-file
validation §7-21), motion trails + symmetry guides, the pen/hints/bind tail, and
the small UI polish rows — **and `PROGRESS.md` itself needs a catch-up pass** before
any of it is scheduled.

---

## 0. The governing rule

> **The window must never stop responding.** Not for a script, not for an export,
> not for a heavy first frame, not for a file dialog.

Everything below is ordered so each item's dependencies are already built when it starts.
Format-version budget: **19** → compositor (Wave 6), **20** → raster layers (Wave 7),
**21** → light tracks / depth sort / aperture / named angles (Waves 9 + 10b).

---

## Phase A — Foundations & throughput (Part II, Waves 4–5)

### Wave 4 — Task registry *(M)* — **start here; gates everything else**
- One place for long work: progress, cancel, survives closing the document.
- Moves off the UI thread: scripts (§7-32 is resolved *by* this), imports, file dialogs,
  and the 305 ms first frame (§7-155).
- Closes: §7-32 (done in Wave 4 per record — verify row struck), §7-155, unblocks §7-81.

### Wave 5 — Background export: queue, presets, Tasks panel *(M)* — depends on 4
- Export **queue** instead of one slot; named presets; global Tasks panel;
  prompt before quitting throws an export away.
- Adds **GIF / WebP** output (CP-6.3 territory).
- Feeds §7-81 (asset thumbnails) and Wave 8.

### Small wins that ride on Wave 4 *(S)*
| Item | Ref | Note |
|---|---|---|
| Assets panel thumbnails | §7-81, §7-22→158 | Reuse Library thumbnail machinery; I/O now lives off-thread via TaskRegistry. |
| Asset drag-drop lands under pointer | §7-82 | Symbols already drop correctly (Wave 1.3); assets merge as whole documents — decide where layers land. |
| Watched assets folder | §7-83 | File watcher thread; no refresh button. Also part of Wave 8 scope. |

---

## Phase B — Look & space (Part II, Waves 6–9)

### Wave 6 — Compositor *(M)* — depends on —
- **Bloom, grain, vignette, grade** as a full-frame pass at the seam where Vello's
  output is blitted (the one place raster passes are legal).
- Same code path on stage and in export → preview *is* result.
- Cheap depth-of-field lands here; format version **19**.

### Wave 7 — Raster layers *(L)* — depends lightly on 4 — the biggest item in Part II
- One canvas per layer; strokes that merge (closes §7-26's remaining vector-brush half);
  working eraser, raster filters, tablet pressure plumbing fed by a real backend.
- Closes §7-164…§7-167 (the four raster limits) and most of §7-25.
- Format version **20**.

### Wave 9 — 2.5D *(M+S+M)* — depends on 4, 6
| Sub-item | Ref |
|---|---|
| Keyframed lights (light rig onto the camera's tween path) | §7-47 |
| Depth sorting of crossing layers/cards (per-frame sort or depth buffer) | §7-60, §7-65 |
| Real depth of field (blur with distance off the focal plane) | §7-29 |

Format version **21** for all three.

### Wave 8 — Asset pipeline: watched folders *(S)* — depends on 4 + thumbnails
Drop a file in → it appears with a thumbnail, no refresh button (§7-83).

---

## Phase C — The film & the camera (Part II, Waves 10b–10)

### Wave 10b — Camera angles *(S–M)* — depends on 9
- An angle is a **camera state**, not a new scene: one new field.
- Angles panel ("Wide", "Close on Ana", "Reverse"); **"Cut to angle at playhead"**
  so a multi-angle sequence lives on one timeline; `Shot.angle` in the project file.
- Honest limit (record it in PROGRESS): flat art seen edge-on is flat — moderate
  pitch/yaw is the believable envelope.

### Wave 10 — The film: `.buzzproj` *(M)* — depends on 5
Many shots, one movie. Shots stay separate files; export queue renders and stitches.
Supersedes §7-12 (multiple scenes) *unless* a reason appears it cannot serve.

---

## Phase D — Delight (Part III). Suggested order: **15 → 11 → 13 → 14 → 12**

### Wave 15 — Command and control *(S)* — do first, immediate payoff
- Command palette on `Ctrl+K` — commands/labels/shortcuts are already data; this is a search box over an existing list.
- Shortcut editor · saved commands for Actions scripts · named version snapshots through the autosave machinery.

### Wave 11 — Animation feel *(M)*
- Motion trails and arcs on stage (bunched ticks = slow motion, lumpy arc = wrong motion).
- Audio scrubbing while dragging the playhead.
- Video reference layer for rotoscoping (ffmpeg already a dependency).
- Frame labels + beat markers detected from the soundtrack.

### Wave 13 — Drawing delight *(M)*
Pull-string stabiliser · symmetry drawing (mirror X/Y, radial) · perspective guides ·
gap-aware paint bucket (Animate's Gap Size) · gradient maps and paper-texture fills.

### Wave 14 — Pro output *(M)*
- **True motion blur at export** — sub-frame GPU accumulation; nothing in the Animate world ships this.
- Alpha video (ProRes 4444 / VP9+alpha).
- Render region · posterise, halftone and hatching in the compositor.

### Wave 12 — Procedural modifier stack *(L)* — **last**: biggest item, better after keyframed lights + motion editor exist
`Wiggle`, `Spring`, `LookAt`, `AutoSquashStretch` on objects and bones; evaluated at draw time, deterministic in `(object, frame)`.
Automatic follow-through/overlap for hair, cloth, tails. (`buzz-physics` already holds the spring integrator + wiggle — this is wiring it into the render path.)

---

## Phase E — §7 long tail not absorbed by any wave

Grouped so each is a session-sized task. Order within phase: **cheap fidelity first**
(import/export correctness), then editor quality, then UI polish last.

### E1. Importers & export fidelity *(highest real-world risk)*
| Item | Ref | Note |
|---|---|---|
| Validate all three importers against real Adobe files | §7-21 | Largest single Phase 5 risk; needs a licensed Animate/Illustrator + real `.fla`/`.xfl`/`.swf`/`.pdf` fixtures. |
| Bitmap import in XFL/SWF/PDF readers | §7-158, §7-116 | Pipeline exists since §7-22 resolution; the three readers still report bitmaps as skipped. |
| SWF: colour transforms on placements (cheapest), then morph shapes, buttons, filters, blend modes | §7-23 | Model already has `ColorTransform`. |
| PDF clipping paths (`W`/`W*`) | §7-24 | Needs a clip concept in the scene model. |
| Sound repeated with picture on export / loop stretch | §7-68 | Arrives with video export; PNG sequence stays silent by definition. |

### E2. Editor tools
| Item | Ref | Note |
|---|---|---|
| Bézier pen — click-drag handle authoring | §7-11 | Anchors already editable via Subselection; this is the drawing gesture. |
| Shape hints for shape tweens | (part of §7-18) | Motion editor shipped; hints are the named remainder. |
| Bind tool — paint skin weights by hand | §7-34 | Deliberately parked in Part I; revisit only when a rig is *wrong* and can't be fixed. |
| Joint speed per bone (Animate's damping) | §7-36 | Small model field + IK read. |
| F6 past end of span duplicates previous artwork, not blank | §7-37 | Phase 3 defect; small fix in timeline command path. |
| Symbol registration point: edit it and let the renderer read it | §7-85 | Stored/saved today but inert. |

### E3. Sound
| Item | Ref | Note |
|---|---|---|
| Sound Properties panel — picker, volume/effect, Event/Start/Stop UI | §7-38 | Model carries all four sync modes + volume; nothing edits them. |
| Make Event/Start/Stop actually differ in playback | §7-39 | Player treats every cue as timeline-positioned today. |
| Proper resampler (not nearest-neighbour) | §7-40 | Fine for animating to dialogue, not a finished mix. |
| Scripting reaches sound (`fl.getDocumentDOM()` → sounds/lip sync/playback) | §7-43 | Phase 8 follow-up; same seam as lights (§7-49). |

### E4. Timeline & document ergonomics
| Item | Ref | Note |
|---|---|---|
| Onion-skin markers as draggable brackets on the ruler (Animate's Onion 2/5/All) | §7-73 | Model is already a range; missing drag + drawn brackets. |
| Edit Multiple Frames: move spans of frames elsewhere | §7-74 | Artwork editing works; frame-moving is unbuilt. |
| Looping section marked on the frame grid, not just the ruler | §7-69 | Band across frames for tall timelines. |
| Nested / per-layer looping sections (playlist becomes a tree) | §7-67 | Real request; touches every frame lookup. |
| Named workspaces (Animate saves several; here one + Reset) | §7-58 | Layout persistence machinery exists. |

### E5. Stage & transform polish
| Item | Ref | Note |
|---|---|---|
| Transform handles on the quad, not its bounding box (rotated objects *and* tilted camera) | §7-61, §7-87 | One gizmo fix serves both rows. |
| 3D rotation gimbal widget on stage instead of sliders | §7-64 | Model identical; missing is the drag + coloured rings. |
| Free Transform handles under a tilted camera (same quad-gizmo work) | §7-61 | Bundle with the row above it. |
| Soft-edged filter bands outlined to fills so width widens toward viewer under tilt | §7-62 | Visible only on steep tilt + large blur; lowest urgency in this group. |
| Drag (not just click/marquee) mapped through inherited transform on followed layers | §7-51 | Same cause as the layer-depth limitation noted there. |

### E6. Filters & colour
| Item | Ref | Note |
|---|---|---|
| Gradient Glow and Gradient Bevel (ramp reaching filter band geometry) | §7-55 | Gradients exist; missing is per-band ramp colours. |
| Remaining four blend modes: Subtract, Invert, Alpha, Erase | §7-54 | Need a compositing model (parent-clip alpha), not one equation each. |

### E7. Swatches & assets
| Item | Ref | Note |
|---|---|---|
| `.clr` / `.act` / `.ase` palette import/export | §7-78 | Retyping palettes is the daily cost today. |
| Drag swatches between folders (dropdown exists) | §7-79 | Polish; same reason Library drag was deferred once. |

### E8. Scripting surface
| Item | Ref | Note |
|---|---|---|
| Scripts saved in document + frame scripts run at their frame | §7-30 | Needs format fields + player hook; panel script is view state today. |
| Widen JSFL subset: text, gradients, tweens, groups, transforms beyond translation, `fl.fileSystem` | §7-31 | Some are editor gaps first — order after E2 where they overlap. |
| Scripting reaches lights (§7-49) with sound (§7-43) — one seam | §7-43/49 | Do together. |

### E9. UI polish *(last; individually trivial, collectively constant)*
| Item | Ref | Note |
|---|---|---|
| Drag panels by title bar into docks (menu already does it) | §7-56 | Drop-zone hit-testing + preview. |
| Camera rotation/zoom direct gesture on stage | §7-16 | Both keyable today; only panning is drag-bound. |

### Parked — named so they are decisions, not oversights
- **Multiple scenes in one file** (§7-12) — superseded by `.buzzproj` (Wave 10).
- **HTML5 runtime export** (CP-6.4) — still on the roadmap; parked until Part II is done.
- **ML-assisted inbetweening / colourisation** — research-grade, heavy dependencies.
- **Collaborative review.**

### Hard limits — do not plan against these
egui pinned 0.35 (blocked on vello/wgpu) · `f64` precision floor ~1e12% · filters & lighting are geometry per-shape (full-frame passes only at the Vello blit seam — that's Wave 6) · no tablet pressure from winit 0.30 on Windows (§7-25 needs a platform backend: Windows Ink / Wintab) · legacy OLE2 `.fla` out of scope · no `.fla` write-back in v1 · egui is immediate-mode (accepted) · Windows-only launcher.

---

## Build order — the whole thing, one line

```
4 → 5 → [81/82/83] → 6 → 7 → 9 → 10b → 10      (Part II: engine)
→ 15 → 11 → 13 → 14 → 12                        (Part III: delight)
→ E1 → E2 → E3 → E4 → E5 → E6 → E7 → E8 → E9   (§7 long tail, fidelity first, polish last)
```

**Every item ships the same way:** section in `PROGRESS.md` §4 (what + why), its §7 row
struck through and marked resolved, its row here / in `IMPROVEMENTS.md` struck with a
pointer — *then* move it out of this plan. An item not written down has not been finished.
