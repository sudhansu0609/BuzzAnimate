# Plan: Animate import fidelity — rigs, tweens, eases, motion tweens, sound

Reported: imported characters have hands and legs "flipped or disoriented based
on the rig"; classic and other tweens "not working"; sound is not imported.
The file-name part of the report is in `docs/UI_SHELL_PLAN.md` §1.

Evidence files (Animate archives; extract with Windows `C:\Windows\System32\tar.exe -xf`,
since Python's zipfile and GNU tar reject them):
`B:/youtubeProjects/Khayal3Baje/stories/bhatta/animate/bhatta2.fla`,
`B:/youtubeProjects/extra/poorane din/sustipur1/Animate/sustipur1.fla`.
Visual harness: `cargo run --release -p buzz-app --example render_symbol -- <fla> "<symbol>" <out_prefix> [frames]`
(set `BUZZ_AT_ORIGIN=1` to place the symbol at its own coordinates;
`BUZZ_EXPORT_NO_CULL=1` to render unculled). Rigged symbols to use:
`sadhuSideWalk`, `OldmanfrontWalk`, `sadhu1FrontWalk` (sustipur1); `pujariFront`,
`pujariSideWalk`, `sadhuSide`, `ThakurBhaiStraight copy 2` (bhatta2).

## Findings (measured on the two files)

| Feature in the files | bhatta2 | sustipur1 | Importer / engine today |
|---|---|---|---|
| Classic tweens (`tweenType="motion"`) | 6,979 spans | 201 | parsed (`parse_tween`, `crates/buzz-import-xfl/src/lib.rs:602`) |
| **Custom ease curves** (`<tweens><CustomEase>`) | 11,766 | 180 | **ignored**; only `acceleration` is read, so eased tweens play linear |
| **Motion Tweens** (`tweenType="motion object"` + `<motionObjectXML>`) | 6 | 14 | **dropped** → `TweenKind::None`; the property curves (including flips such as `Scale_X = -168%`) are lost |
| Shape tweens | 8 | – | parsed |
| `<transformationPoint>` on instances | 12,937 | 574 | imported into `Object::pivot` but **ignored by tweening** |
| Rigged layers (layer parenting) | 266 in 39 symbols | 65 in 5 symbols | resolved + baked (`resolve_layer_parents`, `bake_rig_offsets`, lib.rs:889/988) |
| Sounds (`DOMSoundItem`) | 3 items, 15 frame uses | 2 items | **not imported** (`note_unsupported("sound")`, lib.rs:1600; `sound: None`, lib.rs:1665) |

1. **Classic tweens interpolate about the wrong point, and drop skew.**
   `interpolate_object` (`crates/buzz-scene/src/tween.rs:512`) calls
   `lerp_affine` (tween.rs:618), which decomposes each key into
   translation-of-the-registration-origin, one rotation and two scales.
   Animate's classic tween interpolates the **transformation point's**
   position and the matrix's `scaleX, scaleY, skewX, skewY` separately.
   Measured on bhatta2: joints drift mid-tween by up to 16 px (63 px on
   sustipur1); 141 spans drift more than 2 px and 37 more than 10 px.
   Layer parenting then carries each parent's drift into its children
   (`motion_of`), so the error compounds down arm and leg chains. That is the
   "disoriented" limb. Separately, about 10% of tweened keys carry more than 1°
   of skew, which `decompose` throws away, so the in-betweens snap to a
   different shape than the keys.
2. **Motion Tweens are dropped entirely.** Their animation, including
   horizontal flips expressed as negative `Scale_X`, lives only in
   `AnimationCore` property curves applied on top of the instance's base
   matrix. Imported without them, the part holds its base pose: unanimated and
   possibly **the wrong way round**. That is the "flipped" limb.
3. **Custom eases are ignored.** Every `hasCustomEase="true"` span plays
   linear.
4. **Rig rest-pose hazards (rare).** `bake_rig_offsets` takes the parent's
   rest pose from its first keyframe's *first* object. There are 2 parents
   whose first key is empty and 8 whose first key holds more than one object;
   in those cases the child is baked against the wrong pose on every frame.
   Also, 3 rigged layers change their rig parent over time, while the importer
   keeps a single `rig_parent` per layer.
5. **Sound.** The `.dat` behind each `DOMSoundItem` (`soundDataHRef`) is the
   **original MP3 bitstream**: the files start with the `fffb` MPEG frame sync.
   `sourceExternalFilepath` names the original file as a fallback. Frames
   reference sounds with `soundName`, `soundSync` (`event|start|stop|stream`)
   and `soundLoopMode`/`soundLoop`. The engine already has `SoundLibrary`,
   `SoundCue` and `Scene::add_sound` (`crates/buzz-scene/src/sound.rs`,
   `lib.rs:796`), and `buzz-audio` decodes MP3 through symphonia.

## Ground rules
Same as `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`:
- No `cargo fmt`.
- Don't touch unrelated dirty files.
- Match the comment voice.
- Verify headlessly.
- Use `--release` for anything that renders.

## Phase A — Animate's classic tween model

In `crates/buzz-scene/src/tween.rs`:

A1. **Replace `decompose`/`lerp_affine` with Animate's decomposition.** For a
    matrix `[a b c d tx ty]`, use `scaleX = hypot(a,b)`, `scaleY = hypot(c,d)`,
    `skewY = atan2(b,a)` and `skewX = atan2(-c,d)`, and rebuild with
    `a = scaleX·cos(skewY)`, `b = scaleX·sin(skewY)`, `c = -scaleY·sin(skewX)`,
    `d = scaleY·cos(skewX)`. A mirror is then a 180° difference between the
    skews, exactly as Animate stores it. Interpolate `scaleX` and `scaleY`
    linearly, and `skewX` and `skewY` each the shortest way round. Apply
    `extra_rotations` (Animate's `motionTweenRotate` / `motionTweenRotateTimes`)
    as whole turns added to both skews, with a sign given by CW/CCW. Keep the
    function name `lerp_affine` and its signature (it is `pub`), and grep its
    callers.

A2. **Pivot-anchored interpolation.** Change `interpolate_object` so that, when
    `start.pivot` is `Some(p)` (in the object's local space):
    - the in-between's linear part is the A1 interpolation of the two linear
      parts;
    - its translation is chosen so that `M(t)·p = lerp(M0·p, M1·p, t)`. The
      transformation point travels in a straight line, and the object turns
      and scales about it.

    With no pivot, keep today's behaviour of anchoring at the registration
    origin. Orient-to-path keeps working through the new decomposition.
    Document why in the voice of the file: the joint stays on the joint.

A3. **Rig-relative interpolation.** A child of layer parenting is stored as
    `P_rest · C_rel(f)`. When `P_rest` is not a similarity, interpolating the
    stored matrix is not the same as interpolating `C_rel`. Give
    `interpolate_objects` (tween.rs:226) an optional `frame_of_reference: Affine`.
    When it is set, interpolate `F⁻¹·M0` and `F⁻¹·M1` (and the pivot in that
    space), then left-multiply by `F`. Find where the renderer and the timeline
    resolve tweened frames for a layer (grep `interpolate_objects(`), and pass
    the parent's `rest_pose` for layers whose `follows` is set. Everything
    else passes `None`.

A4. **Tests** in the `tween.rs` tests module:
    - a limb keyed at 0° and 90° about a pivot at its top end: at t = 0.5 the
      pivot's world position equals the keys' (to 1e-9), and the far end has
      turned 45°;
    - a key with 20° of skew survives `lerp_affine(a, a, 0.5)` unchanged (to 1e-9);
    - interpolating between two mirrored keys keeps the determinant negative
      throughout;
    - `extra_rotations = 1` adds exactly 360° to both skews at t = 1;
    - a child under a non-uniformly scaled `F` interpolates in `F`'s space
      (compare with composing by hand).

    Every existing tween test must still pass. If one encoded the old
    origin-anchored behaviour, update it only where the pivot is `None`, and
    list it in the report.

## Phase B — custom eases

B1. `crates/buzz-scene`: extend `Easing` with
    `Bezier(Arc<[Point]>)`, a piecewise cubic Bézier through Animate's point
    list (1 + 3n points from (0,0) to (1,1); `<Point/>` means (0,0)).
    Evaluate by solving x(u) = t for u by bisection plus Newton (monotone in
    x), then return y(u). Also add per-property targets: `CustomEase target`
    is `all | position | rotation | scale | color | filters`, and Animate also
    writes separate `<Ease>` elements with a `method` (quadIn and so on) and a
    `target`. Model a tween's easing as `EasingSet { all, position, rotation, scale, color }`
    so the interpolation in A1/A2 can ease each property with its own curve.
    Where the model makes this awkward, the minimum acceptable version is
    `all` plus `position` / `rotation` / `scale`.
    **Serialisation:** check `crates/buzz-doc/src/serial.rs`/`format.rs` for
    how `Tween` and `Easing` are saved; add the new variants with
    `#[serde(default)]` compatibility so old documents still load.

B2. `crates/buzz-import-xfl/src/lib.rs`: `parse_tween` only sees the
    `DOMFrame` attributes. Also capture the `<tweens>` children
    (`CustomEase`, `Ease`) while the frame is open in `FrameContext`, the same
    way `<elements>` are gathered, and attach them to the pending frame's
    tween when it closes. Map Animate's `<Ease method="...">` names onto the
    existing `Easing` presets where they exist (grep `Easing::` for the
    presets), otherwise onto an equivalent Bézier.

B3. **Tests:**
    - the sample from sustipur1 `Symbol 1.xml`
      (`(0,0) (0.4621,0.0309) (0.7312,0.5053) (1,1)`) evaluates to about 0.08
      at t = 0.5 (compute the true value in the test with an independent
      dense-sampling reference, and assert within 1e-3);
    - an import fixture whose frame has a CustomEase produces that easing on
      the keyframe;
    - old documents without the field load (a serial round-trip test).

## Phase C — Motion Tweens (`motion object`)

C1. `crates/buzz-import-xfl`: when a frame has `tweenType="motion object"`,
    parse `<motionObjectXML><AnimationCore TimeScale=… duration=…>`. Each
    `Property id=…` holds `Keyframe` elements with `timevalue` (in TimeScale
    ticks; `TimeScale="24000"` with a duration of 4,641,000 ticks at 24 fps
    means 1000 ticks per frame, so derive ticks per frame as
    `TimeScale / frameRate`), an `anchor="t,v"` value, and
    `previous`/`next` Bézier handles (`"dt,dv"` offsets). The properties that
    matter: `Motion_X`, `Motion_Y` (offsets in px), `Rotation_Z` (degrees),
    `Skew_X`, `Skew_Y` (degrees), `Scale_X`, `Scale_Y` (**percent change**:
    the sample's -168.153 / 168.153 means a mirrored 268%-ish; verify the
    convention against the file by checking that the resulting frame-0 matrix
    equals the instance's own `<Matrix>` whenever all properties are at their
    rest value, and document what you find), plus `Depth`, colour and filter
    containers (ignore these for v1; list them in the report as unsupported
    with location). `TimeMap` gives the tween's global easing
    (`strength`/`type`). `metadata/Settings xformPtXOffsetPct/YOffsetPct` is
    the transformation point as a fraction of the bounds.
    - **Bake, don't model.** Convert the motion tween into ordinary keyframes.
      Evaluate every property's Bézier curve at **every frame** of the span,
      compose the transform exactly as Animate does (base matrix, then scale
      and skew about the transformation point, then rotation, then
      translation; confirm the order against the frame-0 identity check
      above), and emit a keyframe per frame with `TweenKind::None`. That is
      exact, simple and independent of our tween model. The cost is one small
      keyframe per frame on at most a few dozen layers.
    - If a span is longer than 2,000 frames and a property is linear between
      its keys, emit keyframes only at the property keys, with classic tweens
      between them (linear), rather than one per frame.
C2. **Tests:** a hand-written fixture with a motion tween from x = 0 to x = 100
    over 10 frames, rotating 0 → 90 with a flip (`Scale_X` → negative): the
    matrices at frames 0, 5 and 10 match hand-computed ones. Also run
    `render_symbol` on sustipur1 `fatherFront copy 3` and report whether it
    now faces the other way from before (attach both PNG paths).

## Phase D — rig rest-pose hazards

In `bake_rig_offsets` (lib.rs:988):
- the parent's rest pose is taken from the parent's first keyframe **that has
  an object**, not simply the first keyframe;
- when a parent layer's keyframe holds several objects, use the one Animate
  rigs: the first **symbol instance** on it, falling back to the first object;
- when a child's rig parent changes over time (the frame-level
  `parentLayerIndex` differs between keyframes), bake each keyframe against
  the rest pose of the parent that keyframe names. If `follows` can only
  hold one layer, keep the layer's most common parent as `follows` and bake
  keyframes with a different parent to absolute (world) transforms. Report
  how many layers did this.
- **Tests:** fixtures for an empty first parent key, a multi-object parent,
  and a parent that changes mid-timeline.

## Phase E — sound

E1. `crates/buzz-import-xfl`, when collecting the library
    (`collect_library`, lib.rs:292, and `build_with_media`, lib.rs:406):
    - For each `DOMSoundItem`, read `bin/<soundDataHRef>`. If it begins with
      an MPEG frame sync (`0xFF` followed by a byte with its top three bits
      set), treat it as MP3 bytes.
    - Otherwise try `sourceExternalFilepath`, resolved relative to the .fla's
      folder.
    - Otherwise record a report issue naming the item.
    - Register the sound with `Scene::add_sound` (read its signature and
      `SoundAsset` in `crates/buzz-scene/src/sound.rs`; the asset keeps the
      encoded bytes and a format tag, or decodes through `buzz-audio`,
      whichever the existing Import Sound path does. Grep for the app's
      existing sound import, and do exactly what it does).
    - Map the item name to its `SoundId`.
E2. Frames with `soundName` get a `SoundCue` on that keyframe (lib.rs:1665,
    today `sound: None`):
    - `start_frame` = the frame index;
    - `sync` from `soundSync` (`stream` → the stream variant, so it scrubs
      and lip-syncs; `event`/`start`/`stop` → their equivalents);
    - loops from `soundLoopMode`/`soundLoop`;
    - volume and envelope from `<SoundEnvelope>` if present (else 1.0).

    Do the same for sounds inside symbol timelines if the model supports it,
    and report it otherwise. Remove `note_unsupported("sound")`.
E3. **Tests:** an import fixture with a tiny valid MP3 (generate one at test
    time with the encoder `buzz-audio` already has, if any; otherwise commit a
    few-KB MP3 under `crates/buzz-import-xfl/tests/data/` and say so) and a
    frame referencing it produces one sound asset and one cue with the right
    frame and sync. Then run on bhatta2: `imported.scene.sounds().len() == 3`,
    and the audio plays in the app (headless: decode the first 0.5 s through
    `buzz-audio` and assert it is non-silent).

## Verification (Opus review)
1. `cargo test --release -p buzz-scene -p buzz-import-xfl -p buzz-doc`, plus
   `-p buzz-app --test ease_curve --test direct_sequence --test bone_rig`, and
   every existing tween test.
2. `render_symbol` contact sheets of the seven rigged symbols above, before and
   after, at frames 0, 3, 6 … 24. Joints must stay attached through the
   in-betweens, and no part may flip between consecutive frames unless the
   source does.
3. The joint-drift probe (the method is in this plan's Findings) re-run
   against the engine's own in-betweens: the maximum joint drift is under
   0.5 px on both files.
4. bhatta2 imports with 3 sounds and 15 cues, and the import report no
   longer lists sound.

## 2026-09-25 — barish1.fla: rigged characters come apart (FIXED, measured against Animate)
User report (screenshot, rain scene): heads float off bodies, torsos shrink. Reference: `oldStories/baarish/BaarishFinal.mp4` at 12 s.
- **Cause, partly measured:** `bake_rig_offsets` composes `parent_rest * child` (full matrices). `ThakurBhaiSide copy 2` (the kurta man) hangs its upper torso (stored scale 0.572) under a kurta scaled 0.532 x 0.572, so it renders at about 0.3, half size. Every part in that rig stores the same 0.572, which says Animate does not multiply the parent's scale into the child's size.
- **The file's `rigPropagationMatrix`** (16.16 fixed point, tx/ty in twips) sits on keyframes of rig *parents*, and is a pure rotation equal to `atan2(b, a)` of that layer's own matrix. A mirrored thigh (`a=-0.57, d=0.57`) writes a 180° rotation, and its child shin stores `d=-0.57` to cancel it.
- **Rules tried, none right:**
  - (A) full `P*C`, the current rule: shrinks parts.
  - (B) the child's shape turned by P's closest orthogonal factor, and the child's offset added in the frame of P's translation, rotation and mirror only (no scale): hands detach, the hoe flies off.
  - (E) the child's position through the whole of P, its shape turned only by `atan2(b, a)` of P: the kurta body is fixed and the hoe sits in the hands, but heads and hands still float and the old man splits.
  - The code is back on (A).
- **Fixed, with ground truth from Adobe Animate 2024:**
  - A JSFL script, run on a copy of barish1, detached every rigged layer with `layer.setRigParentAtFrame("", 0)`. Animate keeps each part where it is drawn and rewrites its matrix in world space. `null` is rejected, and `getSelectionRect` returns local bounds, not world.
  - Fitted rule, now `rig_compose`:
    ```text
    world = T(P_world · (C · pivot)) · PROP_parent · linear(C) · T(-pivot)
    ```
    `PROP` is the product of the `rigPropagationMatrix` values (16.16) down the chain, identity where a keyframe has none.
  - `bake_rig_offsets` now stores `inherited_transform⁻¹ · world` per keyframe, so the runtime draws Animate's world exactly at each key.
  - Check (`examples/zz_rig_check`, against `animate_world2.txt`): kurta man 0.17, Ramu 0.10, OldManSideBody copy 5 ≤1.2 px, majdoorside ≤2 px, OldManSideBody copy ≤5 px (palms and hoe only, in symbol space). Before: up to 1366.
  - Visual: all six characters in the rain shot are whole and match `BaarishFinal.mp4`. 2Bhatta2's red-shirt close-up lost a wrongly raised arm and loose hand.
  - Tests: `a_rigged_part_sits_where_animate_draws_it` (Animate's numbers); `-p buzz-import-xfl` 78/78; buzz-app `sequence_import`, `bone_rig`, `rigging_panel`, `import_round_trip`, `rig_exit_test` pass.
  - Residual: palms and tools up to 5 px, likely the `rigPropagationMatrix` tx/ty, which is unused. Between keyframes a child tweens its baked world, not its local, so a parent tweening at the same time can drift slightly mid-tween.
- **Superseded plan:** get ground truth from Adobe Animate 2024, which is installed. A JSFL script opens barish1, dumps each rigged element's stage matrix at a few frames, and exports PNGs. Fit the rule against those numbers, not against guesses. Then run the `render_symbol` sheets of all rigged symbols in barish1, 2Bhatta2 and sustipur1.
- **Fixed 2026-09-25 (verified by render against BaarishFinal.mp4):**
  - (1) Grey slabs. Two causes, both fixed. Pictures that exist only in `bin/*.dat` are now decoded (`decode_bitmap_dat`: the `0x0503` premultiplied-ARGB and `0x0303` palette layouts, with chunked zlib; JPEG/PNG passed through), and barish1 now imports all 21 pictures. Shapes with a `<BitmapFill>` now paint the picture (`Paint::Image`, matrix in twips per pixel, tiled unless `bitmapIsClipped`) instead of flat grey.
  - (2) Opaque rain. Instance colour effects (`<color><Color …/>`: alpha, tint, brightness, advanced) were never imported. Now `parse_color_effect` becomes `SymbolInstance.color`. This also darkens 2Bhatta2 night shots toward the mp4, which were too bright before.
  - Tests: 3 new unit tests, and `-p buzz-import-xfl` passes 77/77.
- **Still open:**
  - (3) Symbol names repeat across "Duplicate Items Folder"s, so `render_symbol` by short name is ambiguous.

## 2026-09-25 (evening) — tweens, eyes, scenes, dimmed pictures (FIXED, checked against Animate)
Ground truth: Adobe Animate 2024 exported 59 frames of barish1 Scene 1 (`exportPNG`, which includes the camera). Our renders went from a worst of 40.7% identical to a mean of **98.3%**, worst 91.6% (frame 100, mid camera push-in; the rest is rain).

1. **Scenes merged.** Every top-level `<DOMTimeline>` was read into one timeline: barish1's Scene 2 layers and camera keys (53 camera keys, where Scene 1 has 3) played over Scene 1. Now `parse_document(…, which)` reads one scene; `ImportReport.more_scenes` carries the rest. `Imported::document()` opens an FLA as a multi-scene document. File > Import into an open document still merges only the first scene.
2. **Classic tweens never moved.** Tweens pair objects by id, but the importer gave each keyframe fresh ids, so every imported classic tween held its first pose. `link_tweened_objects` gives the far keyframe the ids of the objects it tweens from, in order. barish1's digging swing now matches Animate frame by frame (236–254).
3. **Grouped shapes misplaced (the eyes).** A `<DOMShape>`'s own `<matrix>` (grouped shapes, drawing objects) was ignored, so the shape drew at raw coordinates. The left eyeball sat at the symbol's corner and the eye-white mask clipped it to a sliver. It is now applied once; the `<DOMGroup>` matrix duplicates it and stays ignored. The face symbol now renders identically to Animate at inner frames 0 and 39.
4. **Colour effects on pictures.** `Paint::map_colors` leaves images alone, and the overlay it promised was never written, so the night trees' `brightness -0.53` was dropped. `DrawCtx::paint` now draws an image under an effect from a recoloured copy (`c·m + a`, cached per picture and effect, capped at 256).
- Tests: `an_imported_classic_tween_moves_between_its_keyframes`, `a_grouped_shape_is_placed_by_its_own_matrix`, `each_animate_scene_imports_as_its_own_scene`. `-p buzz-import-xfl` passes 81/81, and the app's import, rig and tween tests pass. The render/export suites pass except the known intermittent GPU test (`headless_video`), which passes 3/3 on its own.
