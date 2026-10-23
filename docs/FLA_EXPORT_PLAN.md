# FLA export that Adobe Animate opens (asked 2026-10-22)

User: "Make sure the buzz file can be exported to a fla as well that can be
imported to adobe animate as well."

## What exists

`buzz_export::xfl` (`crates/buzz-export/src/xfl/mod.rs`, with `edge.rs` and
`tests.rs`) is behind File ▸ Export FLA (`Command::ExportFla`,
`App::export_fla_to`, agent `export_fla`).

It already writes:
* the stage (size, colour, fps);
* layers with name, colour, visibility, lock and folders;
* mask and guide layers;
* layer parenting (`parentLayerIndex` / `layerRiggingIndex`);
* keyframes and their spans;
* solid-fill and stroke shapes, groups, and symbols with instances and
  transforms.

It skips the following and reports each one (`FlaReport::skipped`):
* tweens: the keys are written, the interpolation is not;
* sound;
* filters;
* bone rigs: the posed artwork is written, the skeleton is not;
* warp handles;
* gradients;
* bitmap fills;
* inverse masks, which are written as ordinary masks.

It has never been opened in real Animate as far as the docs show.

## Steps

| Step | What | State |
|---|---|---|
| X0 | **Audit in real Animate.** Export these to FLA: ep1 (headless, never saved), a doc made from the AI layer rig (convention names, legs folder, parenting), a doc with Copy Motion keys and tweens, and the sampleNaming.fla import re-exported. Open each in Adobe Animate 2024 with JSFL, on a copy, as the memory "Animate ground truth via JSFL" describes: no real mouse; confirm the run dialog with PostMessage; `doc.close(false); fl.quit(false)`. Dump the layers, parents, folders, keyframes, tweens, symbols and element counts, and export PNGs of chosen frames. Compare with BuzzAnimate's own render (PSNR, not hashes; the renderer isn't bit-stable) and with the source structure. List every failure: refuses to open, opens empty, wrong place, missing layer, lost parent, etc. | done 2026-10-22 (see § X0 findings) |
| X1 | **Fix what X0 found** (opening and structure first). | done 2026-10-23: all four docs re-run in Animate 2024; they open, structure and placement check out. Two small artefacts are still open (see § X1). |
| X2 | **Tweens:** classic tweens with eases (and custom ease curves where Animate's format allows). Shape tweens if the doc has them. | done 2026-10-23: classic/motion → classic tween with `acceleration`, custom eases, rotate; shape tweens. Animate loads all 54,080 of ep1's; motion doc mid-tween 44–46 dB (was 20). Importer's ease sign fixed. See § X2 |
| X3 | **Sound:** audio into `bin/` + `DOMSoundItem` in the library, and the layer's sound with sync/loop. The master audio track goes to a sound layer. | done 2026-10-23: sounds go in as files under `LIBRARY/` (not `bin/`: measured, see § X3), frame sync/loop/repeat/in-out/volume; the Master Audio layer arrives as one stream sound layer, guide layers stay silent. Lengths match in Animate. See § X3 |
| X4 | **Gradients, bitmaps, filters, blend modes, colour effects.** Bitmaps go into `bin/` as `DOMBitmapItem`. | todo |
| X5 | **Rigs:** layer-parented rigs already travel; make sure the AI rig's underlay layers and pivots (transformation points) land right. Bone rigs export as layer-parented rigs (preferred) or as per-frame baked keys, never as lost motion. The camera, lights and effects are baked to keys or artwork, and reported. | todo |
| X6 | **Round trip:** export, open in Animate and save from Animate (JSFL `fl.saveDocument`), then re-import into BuzzAnimate. Structure equal; render PSNR at the floor. Export from the UI and the agent reports what was baked or dropped. | todo |

## X0 findings (2026-10-22, Animate 2024, exporter before X1)

Probe dumps and PNGs: session scratchpad `x0\` (`*.src.txt` = our
structure, `*.animate.txt` = what Animate loaded, `*.anim.f<N>.png` =
Animate's `exportPNG`, `*.buzz.f<N>.png` = our render with the camera off).

* **ep1** (1764 frames, 131 stage layers, 256 symbols): **opens** (~17 min
  for the JSFL dump). All 257 timelines and every layer present; folders and
  layer-parent links all found. **Picture wrong: PSNR 10.5 dB** at frames
  0/300/900/1500 — characters in pieces and upside down, trees as hoops.
  Failures:
  1. Rigged children written with our *absolute* transforms where Animate
     reads *parent-relative* ones, and no `rigPropagationMatrix`: every
     parent applied twice.
  2. Several raw (non-drawing-object) shapes on one frame are one merged
     shape to Animate (Subtitles 76 → 38 elements, Houses 3 → 1), and
     overlapping outlines inside one shape fill by parity (holes).
  3. A masked layer opened unmasked (`ThakurBhaiFaceSide copy 3` ▸
     `eyeballs`): `layerType="masked"` is not read; Animate wants
     `parentLayerIndex` = the mask.
  4. Graphic instances all written `loop="loop"` from frame 0
     (`firstFrame` / single frame / play once dropped).
  5. Folder layers written with a frame; Animate drops it (harmless).
  * Animate-side, not ours: every layer name with a space comes back with
    `_` (`Brow R` → `Brow_R`, 130 layers).
  * Recorded for later: tweens 0 in Animate (X2), gradients (X4: the sky),
    lighting/shadows (X5).
* **motion** (Copy Motion + classic tweens/eases): opens; structure exact
  except tweens 0 (X2). PSNR 45–47 dB on keyframes, 20 dB mid-tween (X2).
* **sampleNaming** re-export: **could not be written** — "Duplicate
  filename: LIBRARY/Tween 1.xml": symbol folders were ignored, so two
  library symbols with one name collided.
* **AI-rigged Guddu** (ep1 + `ai_rig_layers`): same writer paths as ep1.
  Audited in the X1 re-run instead (`rigsolo`: only the rigged Guddu on
  the stage, with the whole ep1 library); see § X1.
* Measured in a hand-built probe FLA: Animate honours the file's
  `rigPropagationMatrix` as written, scale included; with it set to the
  linear part of each parent's own matrix the child lands exactly at
  `parent_world · child`. Without it the child keeps its unrotated shape. A
  missing `transformationPoint` means the centre of the bounds, not (0, 0).

## X1 (2026-10-22)

Fixed in `buzz-export/src/xfl/mod.rs`:
* library paths `folder/name`, unique ignoring case, `DOMFolderItem`s,
  `itemID` on symbols (sampleNaming now writes);
* layer parenting in Animate's convention: child = `parent_world⁻¹ · drawn`,
  `rigPropagationMatrix` on rig parents, dense `layerRiggingIndex`;
* masked/guided layers point at their mask/guide by `parentLayerIndex`;
* shapes beside other elements written as drawing objects; non-zero fills
  with several outlines resolved to their union first;
* graphic instances keep `firstFrame` and loop mode;
* folder layers written without frames.

Our own importer reading the X1 export of ep1: PSNR 12.3 → 20.5 dB (the
rest is lighting and gradients, X4/X5).

### Animate re-run (2026-10-23)

Each export was opened in Animate 2024 by JSFL, on a copy. Session
scratchpad `x0\`: `*x1e.animate.txt` is what Animate loaded and
`*x1e.anim.f<N>.png` is Animate's PNG. `x1e\*.src.txt` and
`x1e\*.buzz.f<N>.png` are our structure and our render (camera off).

The re-run found three more failures, all fixed in `buzz-export/src/xfl`:
* **Fills dropped by Animate** (hair, moustache, trousers, shirt panels).
  * `edge.rs` now writes **whole twips**. A real hair fill written with
    fractional twips opened as slivers in Animate, whether written as
    decimals to 1, 2 or 4 places or as `#hex.frac`. The same edges
    rounded to whole twips opened whole.
  * `edge.rs` also **leaves out segments that rounding collapses**.
    Guddu's eye ring, with 32 sub-twip segments on its inner outline,
    opened as a solid black disc; without those segments it opened as a
    ring.
  * `mod.rs` `drop_slivers`: a fill drops outlines under 1 px². Guddu's
    trouser leg opened with no fill because of three slivers of under
    0.5 px² left by the union step; without them it opened whole.
* **A rig link lost to an empty parent** (`chaiwalaback` ▸ `LeftlegSide`
  → `Leftthigh1`, whose key is empty). In Animate, a child of an empty
  parent is drawn as if the parent were the identity (measured), so the
  link is now written. It is still left out when the empty parent itself
  follows another layer, because that case is unmeasured.
* **Black stage on the sampleNaming re-export.** The fix is in the
  importer (`buzz-import-xfl`): a missing `backgroundColor` means
  Animate's white, not black.

Results with the final exporter (`x1e`):

| Doc | Opens | Structure (layers, types, folders, parents, masks) | firstFrame / loop / matrix / pivot on every first instance | PSNR vs our render |
|---|---|---|---|---|
| ep1 (1764 frames) | yes | 597/597 layers. No differences, except the `_` that Animate puts in names with spaces | 244 checked, 0 off | 19.4 dB at frames 0/300/900/1500 (X0: 10.5) |
| AI-rigged Guddu (`rigsolo`) | yes | 468/468, 0 differences; Guddu's 21 AI layers, legs folder and parents exact | 130 checked, 0 off | 26.2 dB (first X1 export: 17.3) |
| motion | yes | exact | 2 checked, 0 off | 45–47 dB on keys, 20 dB mid-tween (X2) |
| sampleNaming re-export | yes (X0: could not be written) | 677/677, 0 differences | 432 checked, 0 off (one pivot 0.15 px off) | 33.8 dB |

* Masked layers come back with Animate's `parentLayer` set to their mask.
  That is the expected reading, not a difference.
* Folder layers show no frames in Animate, as expected.

What is left is mostly for later steps:
* **ep1's whole picture is darker and cooler in Animate**, and so are
  Guddu's clothes. Our own importer reading the same file renders it the
  same dark way, so this is lighting and colour that the file does not
  carry. It is X5 (lighting, shadows) and X4 (sky gradient, colour
  effects). Cast shadows are missing too (X5).
* Tweens are 0 in Animate (X2).
* **Open, small:**
  * two white dots of about 5 px above Guddu's and Pappu's heads in
    Animate's ep1 render. Our importer reading the same file does not draw
    them, so it is an Animate-side reading; the cause is not found.
  * a few white specks on Guddu's shirt in `rigsolo`.

## X2 (2026-10-23): tweens

Built in `buzz-export/src/xfl/mod.rs` (`tween_attrs`, `easing_points`):
* Classic and motion tweens are written as Animate's classic tween:
  `tweenType="motion" motionTweenSnap="true" keyMode="22017"`. Shape tweens
  are `tweenType="shape" keyMode="17922"`. A tween is written only where a
  next keyframe exists.
* The ease slider is written as `acceleration`. Extra turns are written as
  `motionTweenRotate="clockwise|counter-clockwise"` plus
  `motionTweenRotateTimes`, and orient to path as `motionTweenOrientToPath`.
* Custom curves are written as `hasCustomEase` and
  `<tweens><CustomEase target=…>`. That is either the keyframe's
  `ease_curve` (`all`, or `position`/`rotation`/`scale` with
  `useSingleEaseCurve="false"`) or the tween's own cubic Bézier.
* **`acceleration` is Animate's ease negated** (measured). We wrote
  `acceleration="100"`; Animate's JSFL read `tweenEasing=-100`, and the ball
  was drawn 25% of the way along at mid-span, which is an ease in. The writer
  negates the value. The importer read it un-negated, so every ease in a real
  Animate file came in reversed. `buzz-import-xfl` `parse_tween` now negates
  it too, and its one test was updated to the measured sign. The importer
  also reads `motionTweenRotate="counter-clockwise"` as negative turns.

Measured with Animate 2024 (session scratchpad `x2\`; probe deleted):
* **motion doc** (Ball and Box, eases ±100, a 7-point custom curve, a cubic
  Bézier). Animate read every tween with the right ease, and its
  `getCustomEase` returned our points. PSNR against our render:

  | Frame | 0 | 3 | 6 | 9 | 12 | 15 | 18 | 21 | 24 |
  |---|---|---|---|---|---|---|---|---|---|
  | PSNR (dB) | 45.0 | 44.6 | 44.7 | 43.8 | 47.3 | 45.6 | 45.2 | 44.2 | 45.0 |

  Before X2, frames 6 and 18 were about 20 dB.
* **motion50** (same doc, eases ±50): 26.6–28.6 dB on frames 3–9. This is
  a model difference, not a file one. Animate's classic ease is quadratic,
  `p = t + e·t·(1−t)`, measured on the box (0.344 / 0.624 / 0.844 at t =
  ¼ / ½ / ¾ for an ease of 50). Ours (`Easing::Strength`) is a power curve,
  `1 − (1 − t)^(1 + e)`. The two agree only at 0 and ±100. Open: either
  change `Easing::Strength` to Animate's curve (buzz-scene), or write
  intermediate eases as custom curves.
* **tw2** (rotations, orient, shape tween): clockwise ×1 and
  counter-clockwise ×2 match ours at every mid frame. Two model differences
  remain:
  * Animate ignores orient-to-path without a motion guide; ours orients
    anyway (20.6 dB on that arrow).
  * Animate's shape-tween in-betweens differ from ours (22–25 dB mid-span,
    33–36 dB on the keys).
* **ep1**:
  * Animate loads all **54,080** tweens written: 54,077 classic on the
    stage, plus 3 shape tweens in `fireSymbolTop copy 2`. X1 loaded 0.
  * The structure is otherwise unchanged from X1, apart from one transform
    point that Animate itself reports differently on a shape-tween frame.
  * Interpolated placement was read with JSFL on the copy: every stage
    layer converted to a keyframe at the frame, rig parents detached leaves
    first, world matrices read. At mid-tween frames 374 and 482 (115 tweened
    layers each), the gap between Animate's world matrices and ours was:

    | Frame | Median | 95th percentile | Linear part |
    |---|---|---|---|
    | 374 | 0.35 px | 1.4 px | ≤ 0.008 |
    | 482 | 0.22 px | 0.8 px | ≤ 0.008 |
    | 0 (control, all keyframes) | 0.09 px | 0.6 px | |

  * The one outlier is `Subtitles`: a classic tween over drawing objects.
    Animate classic-tweens only symbol instances, so it holds the key while
    ours scales the text slightly. Open: wrap such keys in a graphic
    symbol on export.
  * Whole-frame PSNR stays at 19.3–19.4 dB, at keys and mid-tween alike;
    lighting dominates it (X4/X5).
* Round trip (export → `buzz_import_xfl`): every written tween came back
  with the same kind, turns, orient and ease (sampled at 5 points, within
  0.006): motion 4/4, tw2 4/4, ep1 54,080/54,080.
* The JSFL dump of ep1 took 41 s, not 17 min, with
  `fl.showIdleMessage(false)`. Without it, Animate sat idle behind its
  slow-script prompt.

## X3 (2026-10-23): sound

Measured first, with hand-built FLAs in Animate 2024 (`x2\sndprobe*.fla`,
exported back with `SoundItem.exportToFile`, and re-saved by Animate):
* **An MP3 in `bin/`** (`soundDataHRef`, with or without `exportNative`,
  `dataLength` or a `LIBRARY/` copy) was read as raw PCM: three minutes of
  noise. Animate's own `.dat` for an MP3 is the MPEG data followed by a
  5 kHz 8-bit waveform cache (`cacheFormat`/`cachedSampleCount`), which we
  cannot write without decoding.
* **The same MP3 as `LIBRARY/<href>`** with no `soundDataHRef` opened as an
  MP3 and exported back byte for byte (the source without its ID3 tag).
  Animate writes its own `bin/` cache when it saves.
* A WAV opened the same whether it was given as raw PCM in `bin/` or as
  `LIBRARY/<href>` (the exports were byte-identical). A 16 kHz WAV was
  converted to 22 kHz by Animate itself. In `bin/`, a 48 kHz clip was
  misread.
* The loop checkbox needs `soundLoop="32767"` beside
  `soundLoopMode="loop"`; without it Animate read no loop. `inPoint44` /
  `outPoint44`, `soundEffect="custom"` and the envelope survive Animate's
  re-save.

So the writer puts each sound in as its file at `LIBRARY/<name>.<ext>`
with a `DOMSoundItem` (`href`, plus `format`/`sampleCount` for the rates
Animate stores), and nothing in `bin/`:
* MP3 and 8/16-bit PCM WAV are written as imported. Other WAV encodings
  are converted to 16-bit PCM. Other formats are reported in
  `FlaReport::skipped`.
* Frames get `soundName`, `soundSync` (event is left off), loop
  (`loop` + 32767) or `repeat` + count, `inPoint44`/`outPoint44`, and a
  `SoundEnvelope` with the volume (`soundEffect="custom"` below full).
* Sound names share the library namespace with symbols and folders (unique,
  ignoring case).
* Master Audio:
  * The stage's master layer is an ordinary layer whose key streams the
    clip, so it arrives as one stream-synced sound layer.
  * `audio_guide` layers inside characters and heads are written without
    their sound, as before.
* `buzz-import-xfl` now also reads a sound from `LIBRARY/<href>`, after
  `bin/` and before `sourceExternalFilepath`. Animate keeps such a copy
  too. It also reads `inPoint44`/`outPoint44` back as the trims.

Verified in Animate 2024:
* **snd_plain**:
  * two items: the MP3 master, and a 16 kHz mono WAV SFX;
  * stage `Dialogue` stream, `sfx` event repeat 3 with in/out and 50%,
    and a loop inside `Cast/Guddu`;
  * every frame read back with the right sync, loop and repeat.
* **snd_master** (the same doc after Enforce Master Audio): one item, one
  stage stream layer of 4327 frames. The guide layer in Guddu has no
  sound.
* Lengths:
  * MP3: Animate's `sampleCount` 7,949,952 = ours, and its export = the
    source frames.
  * WAV: 1,548,543 samples at 22,050 Hz = 70.229 s = the source's
    1,123,660 at 16 kHz.
* Round trip: 3/3 and 1/1 cues came back identical (sync, loops, volume,
  trims, length, rate, channels).

Open: the importer still reads Animate's own repeat (`soundLoop` with
`soundLoopMode` left off) as a single play, and an Animate-saved MP3 `.dat`
with its trailing cache whole (X6).

## Constraints

* No new tests (user rule). Use scratch probes and delete them.
* Never run `cargo fmt`. Don't commit unless asked. Build with `-j 4`.
* Animate runs only on copies in the scratchpad, with no real mouse and no
  SendKeys. Wait for the previous Animate to exit before launching another.
* Never modify the user's FLA/JSFL files. ep1 is loaded headlessly and never
  saved.
* Order: after the running S1/S2 step of `STYLE_CUT_AND_OUTPAINT_PLAN.md`
  (one implementer at a time), X0 → X1 → … → X6.
