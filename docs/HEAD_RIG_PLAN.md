# Head symbol rigs like the user's (asked 2026-10-22)

User: "we need to fix the head Symbol rigs, eyewhites, eyemask, eyeball,
leftEyebrow rightEyebrow mouth eyelids and base, same as in my character."

## The user's head structure (from the FLA dumps)

Layer index 0 is the top.

* `pradhanOldMan/FrontHead`: sound, `eyeLid`, `eyeMask [mask]`, `eyeBall`
  (masked by eyeMask), `eyeWhite`, `rightEyebrow`, `leftEyebrow`,
  `moustacheFront`, `mouth`, `Nose`, `base`.
* `man1Frontt/frontHEadman` and the woman's `head1` are the same shape:
  `eyeLid`, `eyeMask [mask]`, `eyeBall`, `eyeWhite`, the brows, the mouth, and
  `base` at the bottom.
* There is no layer parenting inside heads.
* Each part is its own symbol, and several have their own frames:
  * the eyelid symbol is a blink, about 7 frames;
  * the mouth symbol holds mouth shapes, 10–14 frames;
  * the eyeball symbol has 2 frames;
  * the eye whites and the mask have about 4 frames.
* Both eyes share one symbol per layer (one eyeWhite symbol holds both whites).
* Side heads use `eyeWhite` / `eyeBalls` / `eyeMask` and one visible brow.

Today the AI layer rig only **renames** face layers (N2 Shift+R:
`face_layer_name`). It doesn't build this structure: no eye mask, no eyeball
under a mask, no lid/blink symbol, and no mouth-shape symbol. The ep1 heads
have `Left Eye`, `Right Eye`, the brows, `Mouth` and `Head`.

## Steps

| Step | What | State |
|---|---|---|
| H0 | **Study** every head in sampleNaming.fla, morecharacters.fla and womanSample.fla (front, side, back). Record: layer order, mask links, which symbol each layer holds, frame counts and what the frames are (blink stages, mouth shapes, eyeball directions), how the eye mask relates to the eye white (same outline?), what the eyelid symbol looks like at rest (frame 1 = open = empty?), the pivots, and what the base holds (skin, ears, hair?). Encode it as a `HeadStyle` profile beside `CutStyle`. Sheets go to the scratchpad. | **done 2026-10-22.** 21 heads imported headlessly and rendered: 12 front/side heads with eye rigs, plus 9 back/back-three-quarter heads that are single drawings. For each: every layer alone, every frame of every multi-frame symbol inside it (as strips), and the eye mask against the eye white. `buzz_rig::head_style::HeadStyle::animate_convention()` ("My Animate head style", serde, defaults) holds the result; nothing uses it yet. § H0 findings. Sheets: scratchpad `h0\`. buzz-rig lib 100/100. |
| H1 | **Rig Head** (a command and part of AI ▸ Rig Character as Layers). Turn a head symbol into the user's structure. Split out (or reuse) the eye whites, eyeballs (pupils/irises), brows L/R, mouth, nose, moustache/extras and base. Build `eyeMask` as a mask layer from the eye-white outlines, with `eyeBall` masked under it, `eyeWhite` below, and `eyeLid` on top as a blink symbol: frame 1 open (empty or a lash line, as the user does); further frames closing, made from the skin colour around the eye and the eye-white outline. Make `mouth` a symbol (the existing mouth set / lip-sync shapes if the character has them). Use the user's names and order, with no parenting. The rest picture must be unchanged. One undo step. | **done 2026-10-23.** `crates/buzz-app/src/rigging/head_rig.rs` (`rig_head`, `head_target`, `head_symbol_for`, `flatten_rest`), wired from `rigging.rs` like `style_cut`. Command **Rig Head** (Modify ▸ Timeline next to Rename Face Layers, the palette, the stage right-click on an instance; one undo step "Rig Head"), agent `rig_head {symbol?|element?}`, and "Rig head like my characters" (on by default) in AI ▸ Rig Character as Layers, run in the same undo step as the rig. How it works: face parts found by layer (or object/instance) name; the eye drawings flattened at rest (nested masks applied) and split by colour and place — white = the light fill (both-eye paths split per eye), eyeball = shapes drawn over the white and inside it, narrower than the eye (plus highlights on them, and anything dark drawn over an eyeball), the rest (outline, skin under the white) stays with the white. Mask = the white's outer contours filled (its hull when the white is notched for the iris) less the dark outline that reaches its edge. An iris cut by the eye's edge is completed as a circle *outside the mask only*, under the eyeballs, so rest is unchanged and the circle shows as the eyeball moves. eyeLid = 7 frames at the H0 cover curve (lower edge closing from the top of the eye to its bottom), skin sampled just above/around the eyes on the base, lid line in the eye outline's ink. **Rests open**: the lid instance is single frame on frame 0 (empty) — the user's front heads rest on frame 3 (play once, eyes opening); we chose open, and single frame so a long or root-synced head never blinks by itself. Mouth: an existing mouth-shape symbol is kept; a plain mouth drawing becomes `<head> mouth` (frame 0 = the mouth as drawn). Brows named by screen side. No layer parenting. An already-rigged head (eye mask over masked eyeballs) is left alone. **Measured** (scratch probe, deleted; renders without symbol reuse): ep1's Cast head symbols Guddu_head/Lalten_head/Munna_head are one drawing with the eyes cut out (refused: H2); so the ep1 heads were built from each layer rig's head parts at frame 0 into a head symbol (Mouth, Right/Left Eye, Brow R/L, part 11, Head). Guddu/Lalten/Munna: structure eyeLid > eyeMask[mask] > eyeBall[masked] > eyeWhite > brows > extras > mouth (`<C>_mouth 1`, 10 shapes kept) > base; blink cover per frame Guddu 0/.53/.81/1/.88/.57/0, Lalten 0/.50/.75/1/.82/.53/0, Munna 0/.57/.78/1/.84/.59/0; eyeball moved 10 px left/right/up: 0 changed px outside the mask; one undo restores the symbol exactly. Rest diff on white (whole head 6 px/unit, over24 px of ~0.6 M): Guddu 257 (eyes at 16 px/unit: max 1, 0 px), Lalten 194 (16 px: 164 — its iris was drawn over the eye's outline and is now clipped to the white; its other eye is drawn into the base, so it rigs as a one-eyed side head), Munna 120 (16 px: 950 thin anti-alias lines along the clip; traced pupils that overlap the white's edge stay with the white). OldMan's ThakurBhaiFaceSide copy 3 (already the user's structure): recognised, nothing changed, 0 px. Ground truth (sampleNaming, rest flattened to plain parts — one Eyes drawing, the other layers as groups, lid open — then re-rigged): pradhanOldMan FrontHead and man1 frontHEadman structure equal to the user's (same roles, eye stack, base last; man1's brows sit above the eye stack in the user's head, below it in ours — they don't overlap); IoU white 1.0, mask 0.998/0.999, eyeBall 0.86/0.90 (completed circles vs the user's), lid at the shut frame 0.76/0.75 (the user's lid also covers the socket), brows/moustache/mouth/nose/hair/base 1.0; rest diff 6 px/unit 83/86 px over24 (iris clip seams; the wrapped mouth differs only in edge anti-aliasing). Sheets: session scratchpad `h1\` (`ep1_*.png`, `gt_*.png`, `gt_*_parts.png`, `h1_results.json`). Suites: buzz-rig lib 100/100, buzz-act lib 135/135, buzz-ui lib 375/375 (command count 218→219), buzz-app layer_rig 5/5, ai_agent_coverage 41/41, reuse 6/6. |
| H1b | **Convert a layer-rig head with its animation into a head symbol.** ep1's Guddu, Munna and Lalten have no head symbol with face parts: each head is about 8 layers on the root timeline (mouth, both eyes, brows, head pieces) following the neck, with 470–540 keys of performance, blinks done by swapping eye symbols per key, and lip sync as a per-key mouth `first_frame`. Gather those layers into one head symbol on one Head layer, carrying the motion, blinks and lip sync over, then Rig Head it. (Decided 2026-10-23: H1 handles real head symbols only.) | todo |
| H2 | **Merged/bitmap heads:** when the head is one drawing, cut it into those parts with the same rules, filling the base under the eyes and mouth. Eye whites are completed, and the eyeballs are completed as full circles under the mask. | todo |
| H2-demo | **Full-rig demo (planner, 2026-10-23)**: Guddu (ep1 `rigs/Guddu.buzz`) flattened into one drawing (696 shapes) → AI Rig as Layers (style cut: 16 pieces, 17 layers, user names, layer parenting, legs folder) → head piece wrapped by hand into "Guddu head" → Rig Head (H2 cut). Saved `B:\youtubeProjects\Originals\series\ep1\buzz\rig_demo\Guddu_rigged.buzz` (scratch `examples/zz_full_rig_demo.rs`). **Wrong:** (1) only **1 eye** found (`eyes: 1, eye_holes: 0, view: "side"`): Guddu is a front head with two blank white eyes (no pupils; the Cast heads have eyes as see-through holes or plain white), so the second eye stays in the base; (2) **no brows** (Guddu's brows are drawn into the hair mass); (3) AI Rig as Layers leaves a plain-drawing head as a shape/group piece with `plan.head_symbol = None`, so Rig Head has nothing to run on. | sent to fix (H2a) |
| H2a | Fix the demo's three faults: both eyes on front heads (white eyes with or without pupils, and see-through eye holes; pair by size/height symmetry about the face midline); brows found inside/at the edge of the hair (dark elongated band above each eye, split from the hair mass when joined); AI Rig as Layers makes the head piece a head symbol ("<name> head", identity transform, single frame) whenever the head is not already a symbol, so `rig_head` runs in the same undo step. | **done 2026-10-23** (`head_cut.rs` `find_eyes`/`pick_eyes`/`brow_band`, `rigging.rs` `make_head_symbol` + `AppliedLayerRig.head_symbol/head_made`, `Editor::ai_rig_layers` rigs `applied.head_symbol`; demo's manual wrap removed). Eyes: light fills walled in by paint (a scarf on the drawing's edge is no longer an eye), with or without a pupil, and eye holes, all paired by size/height, a pupil-less pair within one eye spacing of the head's midline. Brows: a dark elongated (or thin arched) fill within 1.5 eye heights above the eye, else a band 0.3 eye high cut off the dark mass's lower edge above the eye (≤ 2 eye heights). **Measured** (`zz_full_rig_demo`, 4×): Guddu head symbol made, front, 2 eyes (2 from holes), 2 brows (cut from the hairline: Guddu draws none), layers eyeLid > eyeMask > eyeBall > eyeWhite > rightEyebrow > leftEyebrow > mouth > base; rest: 37 418 px > 64 of 1.25 M, all but 22 inside the two eye holes now made eyes. Raju: head symbol made but refused (the style cut put Raju's face in `leftArm`; the head piece is scraps), so its head was checked flattened from its own face layers: 2 eyes/2 brows, 3 px outside the eyes (Munna 71, Lalten 22, Woman1 127, all 2 eyes/2 brows). Pictures: session scratchpad `h2a\`. Suites: buzz-rig lib 100/100, buzz-act lib 135/135, buzz-ui lib 375/375, buzz-app layer_rig 5/5, ai_agent_coverage 41/41. Not checked: H1 symbols with separate face parts were not re-rigged in this step (the H1 path was not edited). |
| H3 | **Check against the user's heads:** flatten each user head, re-rig it, and compare layers, masks, frame counts and per-part IoU. Then run the ep1 heads (Guddu, Lalten, OldMan): rest unchanged, blink frames render, eyeballs move inside the mask. Check that lip sync and the director still find the mouth. | todo |

## Constraints

* No new tests (user rule). Use scratch probes and delete them.
* Never run `cargo fmt`. Don't commit unless asked. Build with `-j 4`.
* The user's FLA files are read only. ep1 is loaded headlessly and never saved.
* Order: after S1/S2 of `STYLE_CUT_AND_OUTPAINT_PLAN.md` (the same files in
  rigging.rs), then H0 → H3.

## H0 findings

Measured 2026-10-22 from the extracted copies of the three FLAs (read only),
imported with `buzz-import-xfl` and rendered by a scratch probe (deleted). For
each head the probe rendered:

* the head symbol;
* each layer alone (mask layers drawn as normal layers, Animate's outline
  view off);
* each masked layer clipped by its mask;
* every frame of every multi-frame symbol inside a layer, alone and in the
  head. To get frame f, every placement of that symbol was frozen on f
  ("single frame").

The analysis was a Python script (deleted). Frame numbers are
0-based (Animate's frame 1 = 0).

Heads: pradhanOldMan FrontHead / SideHead14 / HeadBack1 / headOldback;
man1Frontt frontHEadman / headSide / backHead2 / backHead1; man2
manFaceFront1 / sideFaceFront / "manFaceFront1 copy" (back); mazdoor
mazdoorFaceFront / sideHeadMazdoor / HeadbackMajdoor / HeadSideBack3-4;
newman1 frontHead / sideHead / backhEAD; woman head1 / sideHead /
"backHead copy".

Layer order, top first. `[M]` = mask, `[m]` = masked by it, `(n@r)` = n
distinct frames, resting on frame r:

| head | layers | mask ~ white IoU |
|---|---|---|
| pradhan front | eyeLid(7@3) > eyeMask[M](4@1) > eyeBall[m](2@0) > eyeWhite(4@1) > rightEyebrow > leftEyebrow > moustacheFront > mouth(10@3) > Nose > base | 0.77 |
| pradhan side | leftEyebrow2 > Si8deNose1 > moustache > eyesSideTemplate(7@0) > eyeMask[M](4@1) > eyeBalls[m](2@0) > eyeWhite(4@1) > Man2SideMouthDark(15@12) > leftEyebrow2 > sideheadbase | 0.76 |
| man1 front | brow > brow > eyeLid(7@3) > eyeMask[M](4@1) > eyeBall[m](2@0) > EyeWhite(4@1) > moustache > mouth(10@0) > nose1 > [folder] pagdi > frontHair > frontFaceBase | 0.77 |
| man1 side | brow > brow > eyeLid(7@0) > eyeMask[M](4@1) > eyeBalls[m](2@0) > eyeWhite(4@1) > moustache > SideMouthBest(14@12) > topi > base | 0.77 |
| man2 front | brow > brow > mouth(14@13) > eyeLid(7@0) > eyeMask[M](2@0) > eyeBalls[m](1) > eyeWhite(2@0) > hair > nose > base | 0.85 |
| man2 side | brow > brow > eyeLids(7@0) > eyeMaskSide[M](2@0) > eyeballs[m](1) > eyeWhite(2@0) > nose > mouth(18@2) > sideBase | 0.85 |
| mazdoor front | mouth(10@3) > eyeLid(7@3) > eyeMask[M](4@1) > eyeBall[m](2@0) > EyeWhite(4@1) > brow > brow > base | 0.77 |
| mazdoor side | brow > brow > eyeLid(7@0) > eyeMask[M](4@1) > eyeBalls[m](2@0) > eyeWhite(4@1) > mouth(15@12) > base | 0.76 |
| newman1 front | eyeLid(7@3) > eyeMask[M](4@1) > eyeBall[m](2@0) > EyeWhite(4@1) > righteyebrow > mouth(10@3) > leftEyebrow > base | 0.77 |
| newman1 side | brow > brow > eyeLid(7@0) > eyeMask[M](4@1) > eyeBall[m](2@0) > eyeWhite(4@1) > SideMouthBest(14@12) > base | 0.76 |
| woman front | scaryMouth(10@3) > brow > brow > eyeLid(7@3) > eyeMask[M](4@1) > eyeBall[m](2@0) > EyeWhite(4@1) > base | 0.77 |
| woman side | brow(2@1) > brow(2@1) > eyeLid(7@0) > eyeMask[M](4@1) > eyeBalls[m](1) > eyeWhite(4@1) > SideMouthBestPink(14@0) > base | 0.76 |

* **The eye stack never changes**: eyeLid > eyeMask [mask] > eyeBall(s)
  [masked] > eyeWhite, always together. The eyeballs are the only layer the
  mask clips. Brows and the mouth sit above the eye stack in half the heads
  and below it in the rest. This looks the same either way, because they do
  not overlap the eyes. The base is always at the bottom. A hair, turban or
  cap layer, when there is one, sits just above it. There is no layer
  parenting anywhere inside a head.
* **One symbol holds both eyes** on each eye layer (white, mask, eyeballs,
  lid), including the far eye of a side view. **Two brow layers share one
  brow symbol**, the second mirrored or placed again. The woman's side
  brows have 2 frames; every other brow has 1.
* **Eye white and mask**: the white symbol is the white fill plus its lash
  outline (thick on top). The mask is the same eye shape's fill without the
  outline. It lies wholly inside the white (100 %) and covers 0.76–0.77 of it
  (man2 0.85, where the outline is thinner). The two symbols have the same
  frames, frame for frame: 4 eye shapes (normal, wide/round, narrow, squint;
  man2 has 2). Both instances rest on the same frame: firstFrame 1, "single
  frame" (man2: 0). IoU per frame is 0.75 / 0.77 / 0.67 / 0.62, and the mask
  stays inside the white in every frame (≥ 99.7 %).
* **Eyeballs**: one symbol for both eyes, placed "single frame" on frame 0
  inside the mask; 91–100 % of their area falls inside it. Front heads have 2
  frames: a normal iris (0.30 of the white's area) and small, frightened
  pupils (0.14, ×0.46). In side heads the second frame varies (a bigger,
  darker iris). Gaze is **not** a frame: the eyeball symbol slides inside the
  mask. At rest the eyeballs sit within ±0.05 eye-widths of the white's centre.
* **Eyelid**: a 7-frame blink symbol, the same in every head. The share of
  the white each frame covers is 0, 0.50, 0.73, **1.00 (frame 3, closed)**,
  0.79, 0.53, 0. Side heads: 0, 0.41, 0.74, 1, 0.71, 0.43, 0. The lid is
  skin-coloured, with the lash line along its lower edge. **Frame 0 = open =
  (almost) empty**: a speck under 1 % of the white. The lid layer holds the
  blink symbol either directly or inside a timing wrapper (57–133 frames).
  The wrapper places the blink "play once" at the frames where blinks happen.
* **The lid at rest**: 5 of 6 front heads start the blink at frame 3 ("play
  once", firstFrame 3). Frame 0 therefore shows the eyes closed, and they
  open over the next three frames; this is why the front composites look
  asleep. Side heads and man2 rest open (frame 0).
* **Mouth**: one symbol of mouth shapes, set to "single frame". Front heads
  have 10 shapes (11 frames with one duplicate; man2 has 14). Side heads
  have 14–15 (man2 18). The rest frame is 3 for front heads (4 of 6) and 12
  for side heads (4 of 6). Some side mouths nest a "scared" mouth symbol (8
  frames) inside one frame.
* **Base**: the skin face shape with its outline, the ears, eye-socket
  shading, cheek blush, wrinkles and the chin line.
  * It also holds the hair when the head has no hair layer: 4 of 6 front
    heads (pradhan, mazdoor, newman1, the woman) and 5 of 6 side heads.
  * It holds the nose when there is no nose layer (the woman, mazdoor,
    newman1).
  * man2 and man1 keep their hair as separate layers above the base; man1
    also has a turban layer (`pagdi`).
* **Back and three-quarter-back heads are one drawing** (Layer_1 art, or the
  woman's `backBase` symbol): the hair or turban, the ears and the nape skin,
  with no face layers. The woman's back head is her hair with the bun.
* **Pivots** (as a fraction of the head's bounds):
  * The eye group (white, mask, eyeballs, lid) shares one pivot between the
    eyes: about (0.50, 0.51) front, (0.36, 0.52) side.
  * Each brow pivots at its own centre: about (0.70, 0.43) and (0.27, 0.44).
  * Mouth (0.51, 0.79); nose (0.52, 0.61); moustache (0.53, 0.73).
  * The base pivots at its centre (man1 front and newman1 side: at the chin).
* **Nested symbols**:
  * lid → (wrapper →) 7-frame blink;
  * eyeballs → an eyeball symbol (2 frames) → a pupil-highlight symbol;
  * side mouths → a scared-mouth symbol;
  * man1's turban and hair sit in a folder.

`HeadStyle::animate_convention()` (`crates/buzz-rig/src/head_style.rs`)
encodes this:

* the front and side layer lists, each layer with its part, the user's name,
  kind, frames, play mode, rest frame, whether one symbol covers both sides,
  whether every head has it, and its pivot;
* `EyeRig` and `LidRig`;
* the mouth frame counts and what the base holds;
* no parenting.

The front layer order is pradhanOldMan's.

Files in the session scratchpad `h0\`:

* `<head>.png`: the head; each layer alone, tagged with its role; the mask
  (red) against the white (blue); then a strip per multi-frame symbol (blink
  and mouth frames shown in the head, eye shapes and eyeballs alone);
* `heads.json` and `heads.tsv`;
* the raw renders in `h0\raw\`.
