# Style-matched cut + outpaint + rig (asked 2026-10-22)

User: "can the character be cut based on the style I have given in the FLA file?
and then outpaint other parts. I want the AI to rig the characters in the same
way as I rig."

## Goal

Given a character, either a bitmap (traced) or one merged vector drawing:
1. Cut it into pieces **the way the user cuts** in
   `B:\youtubeProjects\Khayal3Baje\stories\sampleNaming.fla`. That covers where
   each cut line falls relative to the joint, what each piece contains (for
   example, is the sleeve with the arm or the body), the shape of the end at a
   joint, how much pieces overlap, and where each pivot sits.
2. **Outpaint** what each piece hides at rest, as the user draws it: the torso
   complete under the arms, the top of the thighs complete under midCloth, round
   limb ends under the parent, and so on.
3. **Rig** with the user's names, parents, folder and stacking (Part N of
   `AI_LAYER_RIG_AND_MOTION_COPY_PLAN.md`, already done), with pivots placed as
   the user places them.

## What exists

* The bitmap pipeline cuts by ownership masks (`buzz-app/src/character.rs`
  ~4254). `buzz_ai::outpaint` extrudes limbs along the bone with a round end
  (`outpaint_limb`), and other parts take `nearest_visible_fill`.
  `TRACE_SPILL_AND_OUTPAINT_PLAN.md` Stage B has the joint cut (B1c) and the
  borrow from the counterpart (B1b).
* Splitting a merged vector drawing (R4–R4d) uses a Voronoi split by capsule,
  panel refinement, overlap copies, inked caps and underlay layers. It is
  generic, not the user's style.
* The user's convention for names, parents, folder and stacking:
  `buzz_rig::pattern::NamingProfile`, `rigging::plan_layer_rig`,
  `apply_layer_rig_plan`.

## Steps

| Step | What | State |
|---|---|---|
| S0 | **Style study.** Import sampleNaming.fla headlessly. For each rigged character symbol (front, side and back views), render every piece alone and measure it against its joints. Measure: the cut line position and angle relative to the joint (fraction of bone length); end shape (round cap / straight / tucked); overlap with the parent (px and fraction of limb width); hidden area (the share of the piece covered at rest, and where); what garment each piece carries (sleeve, collar, cuff, shoe); pivot (transformation point) vs joint; and the stacking. Write a `CutStyle` profile (serde JSON, per slot) built into the code as "My Animate cut style", plus a report with sheets. | **done 2026-10-22.** 8 characters measured headlessly (XFL import, each piece rendered alone and tinted in the composite; joint = centre of the overlap with the parent): man1Frontt manFront/manSideFront/manBack/manSideBack, pradhanOldMan manFront/manSide/man1Back1/back3-4Old; 10 posed pieces (folded left arm, back foot) left out of the medians. `buzz_rig::cut_style::CutStyle::animate_convention()` ("My Animate cut style", serde, per slot) holds the medians; not used yet. Table and spread: § S0 findings. Sheets: scratchpad `s0\*.png`. DWPose not run (joints from overlaps and pivots). buzz-rig lib 100/100. |
| S0b | **Wider style study.** The S0 measurements on the new characters and views of morecharacters.fla (man2 frontBody/sideBody/BackBody: trousers, shoes, a split shirt; mazdoor1/MazdoorSide/mazdoorBack1/MazdoorBack3-4: rolled trousers, sandals; newman1 rameshFrontPose/rameshSide/rameshBackPose: kurta, dhoti, full sleeves) and womanSample.fla (womanChudail1 womanFront/womanSide/womanBack: saree, pallu, hair). What is new: saree, pallu, long hair, trousers (what lowTorso/base pieces hold), shoes, full sleeves, back-view stacking. The user's "<name> copy" symbols, measured against the originals. `CutStyle::animate_convention()` medians over all characters (round elbows kept), stacking per view, and garment rules. | **done 2026-10-22.** 21 character views measured headlessly with one method (each piece rendered alone and with the stack over it; joint = centre of the overlap with the parent), the 8 S0 views included so all the medians come from the same measure. On the S0 characters this method gives S0's hidden share, tip, overlap, cut line, outline and pivot to within about 10 %. The **"copy" symbols are exploded views, not cut-vs-finished art**: each copy places the same part symbols as its original, only pulled apart (rendered areas match to 1.00–1.03). So what the finished rig "adds" is the hidden part of each piece, measured below. `CutStyle` gains `LowTorso` and `Pallu` slots, `views` (stacking per view) and `garments`; § S0b findings. Sheets and TSVs: scratchpad `s0b\`. buzz-rig lib 100/100. |
| S1 | **Style-driven cutter.** Cut by the `CutStyle` profile instead of the generic Voronoi/capsule split, for both inputs: a merged vector drawing (the R4 path) and a traced bitmap (character.rs masks). Cut lines and ends as the profile says; garments go to the slot the profile says; pivots placed as the profile says. The generic split stays as a fallback when no style is chosen. | **vector path done 2026-10-22** (bitmap path not started). `rigging::plan_layer_rig_styled(.., style: Option<&CutStyle>)` and `plan_layer_rig_auto_styled`; `plan_layer_rig_with` = style `None` (R4's generic split, code unchanged); `plan_layer_rig_auto` now splits merged drawings with "My Animate cut style" by default; `LayerRigPlan::cut_style` keeps the choice so `replan_layer_rig` plans again the same way. `MergedSplit` gains `pivots` (per cut slot, placed by the style) and `extras` (a scarf piece: never a lead, named `scarf`). New `buzz-app/src/rigging/style_cut.rs`: fill *panels* (fills joined with no line between) are the units; the capsule labels only vote. Rules, in order: the hips capsule's cells go to the body (the body runs to its hem); **midCloth** = a panel between the thigh bones, below the hips, above the knees, taller than wide; per joint, root first: a panel the child's bone runs through past the style's cut line (60 % of 0.8 L; a limb laid over its parent: of 0.35 L) is the limb crossing the joint - tucked limbs (forearm, hand, shin, foot) are **cut straight, square to the bone, at joint + cut_pos x L** across the limb's run width (feet across the shin, along the shin); a limb laid over its parent (the upper arm in front views) takes the part of the panel inside its own drawn shape (its edges carried up to -tip_beyond x L, closed with its dome), so **the sleeve goes with the arm**; other panels across a joint, or small ones at it (a rolled cuff, a strap), go whole - to the parent when their centre is before the cut line (a half sleeve's rolled cuff stays on the upper arm, a full sleeve's cuff on the forearm) - and only that joint's two sides change hands; a limb folded back along its parent is the parent's; a limb piece showing < 12 % of its capsule is its parent's; **the head ends at the chin** (head panels below the face's lowest point - the face = the largest head panel in the hands' skin colour - go to the body, with the collar and neck); a **scarf** (a colour of its own on the body, hanging from above the shoulders, taller than wide) is an extra of the body stacked over the arms; a line goes to the front-most piece among the fills it joins (it is that piece's outline). Stacking (front view): screen-left leg under the right, thigh over shin over foot, midCloth over the legs, body, arms over the body (screen-left under right; per the style, forearm under upper arm, hand under forearm), scarf, head. Measured: S1/S2 results below. |
| S2 | **Style outpaint.** Complete each piece's hidden area to the profile. Limbs extend under the parent with the user's end shape (vector: extend the shape along the bone with its outline; bitmap: `outpaint_limb`, then trace). The torso is completed under the arms and the hips/thighs under midCloth (vector: continue fills and outlines; bitmap: `nearest_visible_fill` / limb extrude, or the local AI outpaint where it exists). Nothing changes at rest. | **vector path done 2026-10-22** (bitmap path not started). In `style_cut.rs`: every tucked piece is continued under its parent (`limb_extension`): its two silhouette edges near the cut (rows across the bone, ends where no piece stacked over it lies beside them; a hidden edge mirrored about the bone, a thigh's inner edge run to the hips' midline under midCloth) fitted as straight lines, carried back to cut - (cut_pos + tip_beyond) x L and closed with the style's end shape (half ellipse of depth roundness x w/2: thigh 1.75 dome, forearm 0.89 round - the user's choice -, shin/foot/hand round; midCloth straight, from the body's hem); filled from the piece's nearest visible colours (a grid flood through the shape, so stripes carry on) and outlined in the drawing's line colour and width; upper arms laid over the body continue their drawn dome where a piece over them (the scarf) hides them. The torso is completed between its two sides (fitted the same way; a side never seen free is at least as wide as seen) up to its shoulders, plus its hull for the neck under the head. All hidden art is cut to what the piece itself and the pieces stacked over it paint at rest, a grid cell in, and painted under the piece's own art, so nothing of it shows at rest; no underlay layers or joint caps on this path. Measured: S1/S2 results below. |
| S3 | **Rig + UI + agent.** AI ▸ Rig Character as Layers… gets a "Cut style" choice ("My Animate cut style" default when splitting; "Generic"). The bitmap path ("trace a character") uses the same. Agent arg `cut_style?`. One undo step. | todo |
| S4 | **Ground-truth check.** Take the user's own characters from the FLA (pradhanOldMan manFront, man1Frontt manFront, and one side view), flatten each into one merged drawing and into a bitmap, run S1–S3, and compare with the user's real pieces. Report per-piece IoU of the visible and the completed (hidden) area; pivot distance; names and parent tree equal; and the pixel diff of identical bends (each limb ±30°) between the AI rig and the user's rig. Also run an ep1 character (Guddu flattened) for the bend sheets. | todo |

## Constraints

* No new tests (user rule 2026-10-11). Use scratch probes and delete them.
  Existing suites stay green: buzz-rig, buzz-act and buzz-ui lib, buzz-app
  `--test layer_rig`, `--test ai_agent_coverage` and `--test reuse`.
* Never run `cargo fmt`. Don't commit unless asked. Build with `-j 4`.
* Verify headlessly. ep1
  (`B:\youtubeProjects\Buzzcaf_Media\BuzzAnimate\scratchpad\produce\p5\ep\ep1.buzz`,
  sha1 b248835a…) is loaded headlessly and never saved. sampleNaming.fla is
  read only: never modify the user's FLA or JSFL.
* Order: S0 → S1 → S2 → S3 → S4. Every step updates this table.

## S0 findings

Measured 2026-10-22 from sampleNaming.fla (read only), imported headlessly with
`buzz-import-xfl`, frame 0 of each character symbol. A scratch probe (deleted)
rendered the composite, then for each layer the piece alone (its full drawing)
and the composite with that piece tinted magenta (its visible part). Hidden =
full minus visible; the coverer is the top-most piece above it in the stack
whose drawing is there. Joint = centroid of the piece's overlap with its parent
(thighs: with body + midCloth; body: with the thighs; head end of the body: the
head's overlap). Bone length L = joint to the child's joint (hand/foot/head: to
the far tip); w = mean limb width across the middle of the bone. Roundness: 1 =
half circle of radius w/2, 0 = straight, < 0 concave. Units: document px (the
characters are about 1150 tall). Sheets (composite with pivots and parent links;
each piece alone with its hidden part tinted cyan, the green cross = pivot, red
dot = joint, blue = bone) are in the session scratchpad `s0\` (one PNG + TSV per
character).

Characters: man1Frontt manFront, manSideFront, manBack, manSideBack;
pradhanOldMan manFront, manSide, man1Back1, back3-4Old (2 front, 2 side/3-4
front, 4 back/3-4 back). Left out of the medians as posed, not rest: the folded
left arm of man1Frontt manFront/manBack (forearm and hand behind the upper arm),
the back foot of manFront/manBack/manSideFront (behind the shin) and
pradhanOldMan man1Back1's right upper arm (under the gamcha).

Medians (min..max) per slot:

| Slot (layer) | n | hidden at rest | where / under | drawn end (roundness) | hidden end outlined | cut line past joint (×L) / angle | drawn tip before joint (×L) | overlap with parent | pivot | stack |
|---|---|---|---|---|---|---|---|---|---|---|
| Head (`head`) | 8 | 0.00 (0..0.07) | neck end; under body only in man1 back views | round chin 1.04 (0.26..1.31) | 1.00 | −0.09 / 7° | 0.10 | 30 px, 0.22 w | at the chin: 0.11 w in from the drawn bottom | over body 6/8 (man1 back: under) |
| Chest (`body`) | 8 | 0.21 (0..0.37) | sides and neck: under gamcha, arms, head | straight hem 0.15 | 1.00 | −0.12 / 0.5° | 0.12 | 95 px, 0.43 w (with thighs) | 0.46 of L up from the hips (spread: pradhan front/back at the hem, −0.18..−0.07) | root, above the legs folder |
| Scarf (`gamcha`) | 4 | 0.00 | – | round 1.67 | 1.00 | – | – | 310 px, 4.4 w (draped over the body) | 0.44 w below its top end | over body (pradhan only) |
| Shoulder (`leftArm`) | 15 | 0.20 (0..0.58) | top end (0.71): under body (back views), gamcha | dome 2.17 (0.66..2.76), top slanted 45° | 0.54 (0.30..0.68) | +0.07 / 45° | 0.44 | 118 px, 1.84 w | 0.22 w in from the drawn top, 0.36 w to the side | front: over body; back: under (8 under / 7 over) |
| Elbow (`leftWrist`) | 14 | 0.16 (0..0.38) | elbow end (0.95): under the upper arm, or the upper arm's elbow under it | soft 0.42 (man1 0.89 round, pradhan 0.40) | 0.60 (man1 0.98, pradhan 0.57) | +0.13 / 2° | 0.15 | 35 px, 0.65 w | joint (0.06 w from the overlap centre) | under parent 8/14 (pradhan front/side: over) |
| Wrist (`leftPalm`) | 14 | 0.13 (0..0.38) | wrist end (1.00): under the cuff | round 0.94 | 0.91 (man1 1.0, pradhan 0.54) | +0.10 / 4°, cut line straight (0.06) | 0.10 | 17 px, 0.41 w | joint (0.12 w) | under parent 12/14 |
| Hips (`midCloth`) | 8 | 0.18 (0.16..0.33) | top (0.99): under the body's hem | straight 0.21 (man1 −0.09, pradhan 0.64) | 1.00 | +0.09 / 1° | 0.10 | 45 px, 0.86 w | 0.64 w below the top (pradhan 0.31, man1 1.06) | root, top of the legs folder |
| Thigh (`leftThigh`) | 16 | 0.47 (0.34..0.63) | top (0.86): body 0.5–0.9, midCloth 0.1–0.5 | round dome 1.75 | 0.97 | +0.09 / 48° (hem + midCloth edge) | 0.59 | 209 px, 1.61 w | 0.35 w below the drawn top | root; thigh > shin > foot, right leg above left |
| Knee (`leftShin`) | 14 | 0.29 (0.18..0.38) | knee end (1.00): under thigh (+ midCloth 0.1–0.3) | round 1.08 (man1 0.65) | 0.68 (man1 1.0, pradhan 0.62) | +0.17 / 0°, cut line straight (−0.08) | 0.17 | 67 px, 0.64 w | joint (0.06 w) | under parent 14/14 |
| Ankle (`leftFoot`) | 13 | 0.13 (0.09..0.30) | ankle end (1.00): under the shin | round 1.30 | 1.00 (pradhan 0.38) | +0.04 / 26° | 0.18 | 28 px, 0.49 w | joint (0.08 w) | under parent 13/13 |

Garments (by eye on the sheets, checked against the colour clusters):

* **Sleeve with the arm**, never left on the body. Full sleeve (man1Frontt):
  upper arm = sleeve top, forearm = lower sleeve **with the cuff**, hand starts
  at the bare wrist. Half sleeve (pradhanOldMan): upper arm = sleeve with its
  rolled cuff plus the elbow skin below it; forearm = bare skin.
* **Collar and neck with the body**; the head ends at the chin and jaw.
* **Shoe/sandal with the foot**, and the foot carries the ankle.
* **midCloth** = the dhoti's centre fold between the thighs, its top under the
  body's hem (man1Frontt calls it backCloth / Layer_16).
* **Torso drawn complete under the arms**: yes; the sides hidden under the arms
  and the neck under the head are full fill with outline (outline 1.0).
* **Thighs complete under midCloth**: yes; full rounded tops with outline
  (0.97) up under the hem; 0.47 of each thigh is hidden at rest.
* Shins carry the dhoti's lower leg and the bare shin below the hem.

Spread and caveats:

* Pivots: pradhanOldMan puts every pivot on the joint; man1Frontt manFront left
  its leg pivots and its torso pivot at the piece centre (Animate's default)
  and its head pivot at the head's top-left corner. Its other views are on the
  joints. Medians follow the majority, on the joint.
* Arm stacking depends on the view: front views have the arm chain over the
  body, back views under it. Within the chain pradhanOldMan's front/side lay
  the hand over the forearm over the upper arm; everywhere else the child is
  tucked under its parent. Legs are always tucked: thigh over shin over foot.
* Hidden ends: man1Frontt draws every hidden end complete with its outline
  (about 1.0). pradhanOldMan often leaves the hidden end's outline off (shins
  0.62, forearms 0.57, feet 0.38). The style stores the median share.
* Elbow end: man1Frontt round (0.89), pradhanOldMan flattened (0.40); the
  median is the pradhan value (8 of 14 pieces). **User chose round elbows
  2026-10-22**: the Elbow slot of "My Animate cut style" now has man1Frontt's
  round end (`RoundCap`, roundness 0.89), not the pradhan median 0.42, so
  pradhanOldMan's flatter forearm tops score lower against it.
* Visible cut lines across a limb are straight and square to the bone (|angle|
  ≤ 4°, roundness about 0) because they are the parent's hem or cuff. The
  thigh's is the slanted hem plus midCloth's edge (48°).
* DWPose was not run: `detect_symbol_joints` is crate-private to buzz-app.
  Joints come from the overlaps and the pivots, which agree within 0.1 w on
  the distal limbs.


## S0b findings

Measured 2026-10-22 from the extracted copies of sampleNaming.fla,
morecharacters.fla and womanSample.fla (read only), imported headlessly with
`buzz-import-xfl`, frame 0. A scratch probe (deleted) rendered each character
symbol, then each layer alone, folders kept visible and Animate's outline view
switched off. A Python script (deleted) then did the rest:
visible = the piece minus every piece above it; hidden = the rest; coverer =
the top-most piece over each hidden pixel. The joint is the centre of the
overlap with the parent. For thighs the parent is body + body2 + midCloth;
midCloth's is body + body2; a body that follows body2 uses body2. The bone runs
from the joint to the child's joint (hand, foot, head, pallu: to the far tip),
and w is the median width across 0.35–0.65 of it. Outline = the share of the
hidden edge that has the dark line within 3 px. Units are document px.

Check against S0: run on S0's 8 views, this method reproduces S0's medians for
hidden share, drawn tip, overlap, the distal cut lines, hidden outline, pivots
and length/width to within about 10 %. Examples: Thigh 0.47/0.57/223 px vs
S0's 0.47/0.59/209 px; Knee outline 0.68 vs 0.68. It does **not** reproduce
S0's end roundness, cut angles, or the Head and Chest cut lines (different
definitions), so `CutStyle` keeps the S0 values for those. Exceptions: Chest,
Thigh and Ankle roundness, which agree, take the 21-view median (0.17, 1.64,
1.30). The elbow stays round (0.89) by the user's choice.

Characters (21 views): the 8 of S0, plus man2 front/side/back, mazdoor
front/side/back/back34, newman1 front/side/back, and the woman front/side/back.
Medians over all 21 (min..max), posed pieces left out as in S0:

| Slot | n | hidden | drawn tip before joint (×L) | overlap px / ×w | hidden outline | pivot | L/w | stack in the arm/leg chain |
|---|---|---|---|---|---|---|---|---|
| Head | 21 | 0.00 (0..0.07) | 0.13 | 37 / 0.30 | 1.00 | 0.07 w in from the chin | 1.12 | over body 19/21 |
| Chest (`body`, torsoTop) | 21 | 0.12 (0..0.47): sides under the arms, neck under the head | 0.13 | 96 / 0.48 | 1.00 | **at the waist**, 0.03 L up (S0 chars 0.46; every new one ≈0) | 1.72 | root, or follows body2 |
| LowTorso (`body2`) **new** | 6 | 0.23 (0.05..0.37): top under the body | 0.20 | 81 / 0.46 | 1.00 | at its hem, 0.92 L | 1.56 | root |
| Scarf (`gamcha`) | 7 | 0.00 | – | 321 / 4.69 | – | (S0) | 2.59 | over body 7/7 |
| Pallu **new** | 3 | 0.68 front/side, 0 back | 0.67 | 451 / 5.34 | 1.00 | 0.18 w below its top, on the shoulder | 2.76 | front/side: bottom of the stack; back: over body |
| Shoulder | 42 | 0.20 (0..0.67) | 0.36 | 112 / 1.78 | 0.53 | 0.18 w in from the top | 1.90 | – |
| Elbow | 40 | 0.08 (0..0.38) | 0.10 | 28 / 0.60 | 0.67 | joint | 2.75 | **over the upper arm 24/40** (S0 chars 6/14) |
| Wrist | 40 | 0.00 (0..0.37) | 0.12 | 19 / 0.50 | 0.66 | joint | 1.92 | over the forearm 23/40 |
| Hips (`midCloth`, `base1`) | 14 | 0.29 (0.16..0.99) | 0.13 | 63 / 1.20 | 1.00 | 0.77 w below the top | 3.94 | root (base1 follows body2) |
| Thigh | 36 | 0.45 (0.29..0.84) | 0.48 | 175 / 1.44 | 1.00 | 0.31 w below the top | 1.29 | root |
| Knee | 35 | 0.25 (0..0.50) | 0.15 | 59 / 0.68 | 0.66 | joint | 2.45 | under the thigh 28/35 |
| Ankle | 40 | 0.13 (0..0.75) | 0.18 | 28 / 0.48 | 1.00 | joint | 1.48 | under the shin 37/40 |

Distal cut lines (visible edge past the joint, ×L): Shoulder 0.07, Elbow 0.10,
Wrist 0.09, Hips 0.10, Thigh 0.18, Knee 0.15, Ankle 0.10, LowTorso 0.18.

### What is new

* **Saree** (woman, 3 views). `body2` (LowTorso, the root) is the saree from
  the waist to the feet, with its green border and, in the back view, the
  pleated back drape. Its top is a dome hidden under the blouse (0.05–0.06,
  front/side) or under the pallu (0.11, back). `body` (Chest, follows body2)
  is the blouse with the saree's drape across the chest and the neck; its
  sides and neck are hidden (0.06–0.10). In the back view `body` (blouse back
  and bare waist) lies **under** body2 and is 0.47 hidden. There are no
  thighs or shins: both feet follow body2, with domed ankle tops hidden 0.17–0.53
  under the hem.
* **Pallu**: its own layer, following body2 (root in the side view). In front
  and side views it is at the **bottom of the stack**, 0.68–0.77 hidden behind
  body and body2; only the strip hanging past the shoulder shows. Its top is
  drawn as a long rounded panel with outline 1.0. In the back view it is second
  from the top (under the head), laid over the back, and hidden 0. Pivot: 0.18
  w below its top, on the shoulder.
* **Hair**: always in the head piece. The woman's bob is in the head's base,
  and the back head is the hair with the bun. Nothing hangs onto the body;
  the head stays over the body.
* **Trousers** (man2). The shirt is split at the waist: `body` = torsoTop
  (collar to the waist, follows body2) and `body2` = torsoBottom (waist to
  the hem, root), whose top is hidden 0.35–0.37 under torsoTop.
  There is **no midCloth**. `base1` (Hips slot) is a dome-shaped crotch piece
  (shirt-coloured top, trouser-coloured bottom) that follows body2 at the very
  bottom of the stack and is 0.89–0.99 hidden at rest; it fills the crotch
  when the legs spread. The thighs are the trouser leg from hip to knee,
  **domed at both ends**, with the top hidden 0.37–0.52 under body2. In man2
  the shin lies **over** the thigh and hides the thigh's round knee end. The
  shins are the lower trouser leg down to the hem. Mazdoor (rolled trousers)
  has no LowTorso and no crotch piece. Its shirt with the sash covers the
  thigh tops (0.29–0.41), the thigh lies over the shin, and the shin carries
  the rolled cuff and the bare shin. newman1 (kurta and dhoti) has a midCloth
  like pradhan's, 0.32–0.40 hidden under the long kurta, with puffy dhoti
  thighs 0.63–0.65 hidden.
* **Shoes** (man2): the foot piece is the shoe. It sits under the shin, with
  its top band hidden 0.22–0.37 under the trouser hem, a rounded top, and
  outline 1.0. One front shoe (rightShoe) sits above the legs folder, over
  its shin, with nothing hidden. Sandals (mazdoor, newman1) carry the bare
  ankle and are hidden 0.06–0.12.
* **Full sleeve** (newman1's kurta): the upper arm carries the sleeve to the
  elbow. The forearm carries the **rolled cuff at its top**, hidden 0.24
  under the sleeve, then the bare forearm. Half sleeves (man2, the woman's
  blouse, mazdoor) are as pradhan's: the upper arm = sleeve + elbow skin,
  with its round elbow end hidden (0.07–0.20) under the forearm laid over it.
* **Stacking per view** (`CutStyle::views`; majorities over 6 front, 6 side,
  6 back and 3 back34 views):

  | view | arms vs body | forearm / upper arm | hand / forearm | shin / thigh | foot / shin | head over body | midCloth over thighs | pallu | right leg over left |
  |---|---|---|---|---|---|---|---|---|---|
  | front | over 12/12 | **over 9/12** | over 10/12 | under 8/10 | under 7/10 | 6/6 | 6/8 | under all | 3/5 |
  | side | near arm over, far arm under (6/6) | **over 12/12** | under 7/12 | under 8/10 | under 10/10 | 6/6 | 6/8 | under all | 3/5 |
  | back | under 11/11 | under 9/11 | tie 6/6 | under 7/10 | under 10/10 | 5/6 (man1: under) | 8/8 | over the body | 4/5 |
  | back34 | under 6/6 | under 6/6 | under 4/6 | under 6/6 | under 6/6 | 2/3 | 4/4 | – | left over 3/3 |

  In back views the upper arm's inner strip is hidden under the body
  (0.17–0.61, along its length). The woman's back has body2 (the saree) over
  the blouse; man2's back has the shirt bottom under the top.
* **Kept in the code for S1**: the slot `stacking` fields (forearm and hand
  tucked) are unchanged, because S1/S2 complete only the tucked proximal ends.
  The measured front and side views lay the forearm and hand **over** their
  parent, which hides the parent's distal end instead. Switching needs a
  distal completion in S2 first.

### The "copy" symbols are exploded views

Every `<name> copy` (frontBody/BackBody/sideBody in morecharacters.fla;
womanFront/womanSide/womanBack in womanSample.fla) places **the same part
symbols** as its original, with the same layers and parents; only the
instance matrices differ (11–16 per pair). Rendered piece areas match to
1.00–1.03. So the copy is the finished rig pulled apart, and the "finished"
rig adds no drawn art. What it does is slide each piece back along its bone,
by a median of 88 document px relative to its parent (40–340; the pallu
furthest), until its drawn end goes under its neighbour. The hidden part
measured on the assembled rig is exactly what shows in the exploded copy.
Carried over to the copy by each piece's offset, it is visible there
(`shows_in_copy` 1.00 for every piece except man2 front's body2, whose top
stays under torsoTop in the copy as well). Sheets: `s0b\pair_<view>.png`,
assembled (magenta = hidden at rest) next to the copy (the same areas
carried over).

Per piece, what the assembled rig hides (= what each piece is drawn beyond
its visible cut):

| pair | piece | hidden | where | shape | outlined |
|---|---|---|---|---|---|
| man2 front | upper arm | 0.08 | elbow end, under the forearm | round | 0.17 |
| | forearm | 0.08 | wrist end, under the hand | round | 1.00 |
| | torsoTop | 0.12 | neck under the head, sides under the arms | – | 0.98 |
| | torsoBottom | 0.36 | top, under torsoTop | straight, round corners | 1.00 |
| | base1 | 0.99 | all of it | dome | 1.00 |
| | thigh | 0.37 | top under body2, knee under the shin | dome both ends | 1.00 |
| | shin | 0.04 | ankle end, under the shoe | round | 1.00 |
| | shoe | 0.18 | top band, under the hem | round | 1.00 |
| man2 side | upper arm | 0.20 | elbow end | dome | 0.84 |
| | thigh | 0.52 | top + knee | dome | 1.00 |
| | torsoBottom / base1 / shoe | 0.35 / 0.98 / 0.23 | as front | | 1.00 |
| man2 back | upper arm | 0.12 | top inner side, under the body | dome | 0.52 |
| | forearm | 0.23 | elbow end, under the upper arm | round | 0.68 |
| | thigh / torsoBottom / base1 / shoe | 0.41 / 0.37 / 0.89 / 0.28 | as front | | 1.00 |
| woman front | upper arm | 0.11 | elbow end, under the forearm | dome | 1.00 |
| | forearm | 0.07 | wrist end, under the hand | round | 0.76 |
| | body (blouse) | 0.10 | neck, sides | – | 1.00 |
| | body2 (saree) | 0.06 | top, under the blouse | dome | 1.00 |
| | feet | 0.45 | ankle, under the hem | dome | 1.00 |
| | pallu | 0.77 | all but the strip past the shoulder | long rounded panel | 1.00 |
| woman side | upper arm | 0.29 | top, under the body | dome | 1.00 |
| | forearm / body / body2 / feet / pallu | 0.06 / 0.06 / 0.05 / 0.45 / 0.68 | as front | | 0.56–1.00 |
| woman back | upper arm | 0.48 | inner side, under the pallu and body | dome | 1.00 |
| | body (blouse back) | 0.47 | waist under body2, neck under the head | dome | 1.00 |
| | body2 | 0.11 | strip under the pallu | – | 1.00 |
| | feet | 0.17 | ankle | round | 1.00 |

Hands, heads and the back pallu hide nothing. The gaps close by overlap
only: no piece is redrawn between the two symbols.

Files in the session scratchpad `s0b\`: `<view>.png` (composite with joints,
pivots and bones, then each piece alone with its hidden part in cyan),
`<view>.tsv` (per piece), `s0b_all.tsv`, `medians_all.tsv` /
`medians_new.tsv` / `medians_s0chars.tsv`, `stacking.txt`, `pairs.tsv`,
`pair_*.png`; raw renders in `s0b\raw\`.


## S1/S2 results

Measured 2026-10-22 with a scratch probe (deleted; PNGs kept in the session
scratchpad `s1\`). Ground truth = the user's own characters from
sampleNaming.fla (the extracted copy, read only), imported headlessly. Each
symbol's pieces were flattened into one merged drawing the Animate way (each
opaque colour cuts the colours under it and joins its own, then split into
disjoint regions; translucent paints kept over what they shade; no strokes
were present, all lines are fills). DWPose ran once on the merged drawing;
the split was then planned and applied with style `None` (R4 generic) and
with "My Animate cut style". Each AI piece is matched with the user's piece of
the same part type (best visible IoU). Visible = the piece's pixels not
covered by layers above; full = the piece rendered alone (hidden part
included). Pivot = the layer object's transformation point. Rest diff = the
AI rig against the merged drawing it was cut from (per-pixel largest channel
difference, on white). Bend diff = each limb turned +30 and -30 degrees about
its own pivot in the user's rig and in the AI rig (the rest of the rig
following by layer parenting): mean over the image, and the share of the
figure's pixels differing by > 24. That share includes the baseline difference
between the merged drawing and the user's own composite (0.096 pradhan front,
0.063 man1 front, 0.100 pradhan side, already at rest: anti-aliasing where
every colour region of a merged drawing is cut out of the others), so only
the excess over it is the rig.

| character | split | mean IoU visible | mean IoU full | median pivot px | rest diff vs merged (max / mean / px > 24) | bend mean / share > 24 |
|---|---|---|---|---|---|---|
| pradhanOldMan manFront | generic | 0.695 | 0.658 | 20.7 | 230 / 0.047 / 285 | 5.41 / 0.127 |
| pradhanOldMan manFront | **style** | **0.881** | **0.848** | 26.0 | 227 / 0.081 / 594 | 5.57 / 0.131 |
| man1Frontt manFront | generic | 0.541 | 0.487 | 41.5 | 162 / 0.068 / 270 | 5.86 / 0.119 |
| man1Frontt manFront | **style** | **0.711** | **0.655** | 48.2 | 183 / 0.088 / 453 | 6.41 / 0.122 |
| pradhanOldMan manSide | generic | 0.662 | 0.647 | 22.1 | 215 / 0.054 / 272 | 6.75 / 0.133 |
| pradhanOldMan manSide | **style** | **0.892** | **0.836** | 22.6 | 205 / 0.088 / 515 | 7.20 / 0.140 |

Per piece (style; visible / full IoU / pivot px): pradhan front: head
0.99/0.99/14, body 0.92/0.85/151, scarf (gamcha) 0.98/0.98/221, midCloth
0.92/0.87/23, upper arms 0.78/0.78/55 and 0.92/0.75/19, forearms 0.86/0.82/21
and 0.85/0.82/16, hands 0.96/0.92/12 and 0.94/0.93/6, thighs 0.85/0.62/51 and
0.78/0.70/32, shins 0.81/0.85/40 and 0.74/0.83/29, feet 0.87/0.91/2 and
0.92/0.93/34. man1 front: head 1.00/1.00/213, body 0.95/0.91/48, midCloth
0.95/0.90/7, upper arms 0.83/0.83/8 and 0.74/0.74/26, forearm 0.67/0.72/45,
hand 0.96/0.95/6, thighs 0.91/0.63/142 and 0.96/0.72/132, shins 0.89/0.79/80
and 0.91/0.79/90, foot 0.92/0.83/46. pradhan side: head 0.99/0.99/16, body
0.93/0.84/44, scarf 0.97/0.97/227, midCloth 0.94/0.91/22, upper arms
0.85/0.81/48 and 0.90/0.63/11, forearms 0.88/0.76/27 and 0.85/0.85/16, hands
0.91/0.92/7 and 0.96/0.93/12, thighs 0.82/0.68/58 and 0.88/0.56/116, shins
0.78/0.84/24 and 0.84/0.87/18, feet 0.88/0.87/10 and 0.91/0.94/31.

Names and parents: the AI's parent tree equals the user's by part (head,
scarf and arms follow the body; the arm and leg chains; body, midCloth and
thighs are roots; a legs folder). The names are the profile's, so the user's
own suffixes and typos differ (FrontHead/head, gamcha or Layer_20/scarf,
midCloth1, rightThgh, rightShin1, leftFoot2, leftPlam2, tttorso1, Layer_16,
frontHEadman, leftArm1). man1 front's folded left forearm and hand and its
back right foot are hidden behind other pieces in the user's drawing: nothing
of them is in the merged drawing, so the AI has no such pieces (IoU 0).

Where it falls short:

* **Thighs' hidden part** (full IoU 0.56-0.72): the user's thigh dome is
  lower and wider than ours. The dome's height comes from DWPose's thigh
  length, and DWPose puts a baggy dhoti's knee lower than the user's thigh
  bottom (our thigh also reaches lower at the knee cut).
* **Pivots**: the style's medians do not fit the user's outliers. pradhan's
  body pivot sits at the hem (style: 0.46 up the torso, 151 px off); man1
  front's head pivot is at the head's corner and its leg and torso pivots at
  the piece centres (Animate's default); the scarf takes the body's pivot
  (one pivot per slot in the plan; the user's gamcha pivot is on the
  shoulder). Distal pivots are 2-45 px off.
* **Forearm tops**: the user chose round elbows; pradhan draws flat-topped
  forearms laid over the upper arm, so its forearms score lower on the full
  shape (0.76-0.85) than its hands. man1's forearm (full sleeve, no line at
  the elbow) is cut at DWPose's elbow + 0.13 L and scores 0.67/0.72.
* **Rest picture**: 450-600 px per character differ by > 24 from the merged
  drawing (max 183-227): single anti-aliased pixels along boundaries where
  two pieces now stack in a different order than the merged drawing's
  colours (the chin line over the neck, the collar, the body's hem over the
  thighs). The hidden art is not the cause (a 2-cell inset left the count
  unchanged). R4 generic on the same drawings: 270-285 px.
* **Bends**: the bend figures stay close to the generic split's and are
  dominated by the merged-vs-user baseline (0.06-0.10 of the figure). On the
  sheets, raising an upper arm shows the torso continued to its shoulder but
  without the side outline where the arm's silhouette clipped it, and a few
  slivers of sleeve edge stay on the body; feet show small chips at the
  ankle when turned.
* Back views (arms under the body) are not handled: the stacking is always
  the front view's.

**ep1 Guddu** (loaded headlessly, never saved, sha1 b248835a... unchanged; its
20 parts flattened to 837 colour regions): the style split gives 14 pieces
and a legs folder (no midCloth: trousers); rest vs merged max 136, mean
0.051, 443 px > 24 (generic 148 / 0.036 / 178). Guddu's own rig is not cut
the user's way (its shoulder pieces hold the shirt halves), so IoU against it
is not ground truth (mean visible 0.63 style, 0.67 generic). Bend sheets:
shoulders, elbows, wrists, hips, knees and ankles bend without holes and the
short sleeve now turns with the arm; raising an arm leaves a few texture
specks under it. **Separate pieces**: ep1 Guddu and the user's pradhanOldMan
manFront planned with the default (style) and with `None` give equal plans
(merged_found 0, no split) and identical applied layers.

Sheets (`s1\`): `<character>_<split>_pieces.png` (top: user composite, AI
composite, rest diff; then per part user | AI with hidden parts tinted cyan
and the pivot in green | full-shape overlay, red = user only, blue = AI
only, purple = both), `<character>_<split>_bends.png` (each limb +-30
degrees: user | AI), `_rest.png`, `_rest_diff.png`, `<character>_user_rest.png`,
and for the style split `_panels.png` / `_labels.png` (the grid's panels and
final labels). Tests (no new tests): buzz-rig lib 100, buzz-act lib 135,
buzz-ui lib 375 (2 ignored), buzz-app layer_rig 5, reuse 6,
ai_agent_coverage 41 (2 ignored; every_menu_command skipped): all green.
