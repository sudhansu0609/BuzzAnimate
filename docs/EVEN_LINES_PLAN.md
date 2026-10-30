# Even lines after a trace (asked 2026-10-23)

The user: "after the character is traced, can the outlines be changed so it
looks made with the line tool — uniform thickness all through the line."
Then: "face lines thinner than body ones. I shall also be able to control."

Ink and Ink + colour traces give the ink as **filled shapes** whose thickness
follows the drawing (a brush look). This turns them, after the trace, into
real **strokes of one width along each line**, with face lines and body lines
each at their own width, both set by the user.

## Status

| Step | What | State |
|---|---|---|
| E1 | Core: `buzz_scene::even_lines` — ink shapes → even strokes | done (code): `crates/buzz-scene/src/even_lines.rs` `even_lines`, `measured_line_width` (median `2r-1` on the EDT ridge), `EvenLines::set_widths` (+ `measured` per stroke, for instant re-width); `trace_lines::trace_core_with` + `Straighten` hook (default = Trace Bitmap unchanged); `bucket::rasterise_fill` pub(crate). `cargo test -p buzz-scene --lib` 549/549 |
| E2 | Command **Even Out Lines…** with dialog, live preview, agent action | done (code): `Command::EvenLines` id `even_lines` (Modify ▸ Shape, palette, stage right-click); `buzz-ui/src/even_lines_dialog.rs`; `buzz-app/src/rigging/even_lines.rs` `plan_even_lines` / `trace_even_lines` / `apply_even_lines`; `Editor::open_even_lines`, `even_lines_action` (preview = apply over the originals; Apply = `collapse_run` → one step; Cancel = collapse + undo), `even_out_lines`; agent `even_lines` in `agent/dialogs.rs`. buzz-ui lib 375/375 (count 220), ai_agent_coverage 41/41. Probe `examples/zz_even_lines_probe.rs` on Guddu.buzz (scope = Guddu symbol): 19 face / 115 body lines, 21 solids, 1.49 s; outlines joined to hair/fat ink stay fills (see E3) |
| E3a | First look (planner, 2026-10-23) | **sent back.** Guddu probe (`scratchpad/e3/guddu_pair.png`, `face_pair.png`): ears, nose, mouth, pocket and folds are even strokes, but the face outline joined to the hair and most of the silhouette are unchanged. Cause: `find_masses` makes a whole connected component a mass when any pixel is deeper than the radius. Suites green (scene 549, ui 375, agent 41). 1.49 s per character, 668 ms draft retrace. |
| E1b | Fix: split masses by opening; dark-luma rule only without Ink layers; faster draft; Cancel leaves no redo | done (code, implementer 2026-10-23): `trace_lines::trace_core_opened` (+`opened_masses`, tuck band 0.5×line, drop chains inside mass; Trace Bitmap path unchanged); `plan_even_lines` keeps only Ink layers when any in scope; draft 3 px + per-set cache `retrace_even_lines`; `Document::restore_checkpoint` for Cancel. Guddu probe (`scratchpad/e3/round2/`): Ink area replaced by strokes 0.17 → 0.81, 76 face / 216 body lines, 44 solids, full trace 181 ms, cold draft 69 ms, solid-above draft step 64 ms; face outline is a stroke up to the hair, hair a fill. Suites: scene 549/549, ui 375/375 (first run hit the known flaky shift-click test, rerun green), agent 41/41. Open: small blobs where a line thickens (mouth corners, jaw by the ears) |
| E3b | Second look (planner, 2026-10-23) | **mostly good, three defects sent back.** `scratchpad/e3/round2/face_pair.png`, `guddu_pair.png`: the face outline and body silhouette are even strokes now (stroke area 0.17 → 0.81; 181 ms full, 69 ms draft). Defects at 400 %: (a) small leftover black blobs where a line thickens past Solid above (both jaw/ear joins, left temple, mouth corners); (b) the **mouth's dark inside** (a filled crescent with the teeth) got thinned to a line, which changes the drawing; (c) a stray thin spur hangs off the right jaw. |
| E1c | Fix (a)–(c): small masses fold into lines; filled shapes clearly thicker than lines stay fills whole; prune spurs | **partial** (implementer 2026-10-23): (a) `trace_lines::fold_small_masses` (opened path only) and (c) `prune` spur limit 1.5 × 2r at the junction (opened path only) work: jaw/ear blobs and the right-jaw spur are gone in `scratchpad/e3/round3/guddu_face400_after.png`. (b) `keep_filled_forms` in `plan_even_lines` (shape median > 1.8 × set median stays as drawn) is built but **does not catch the mouth**: its two crescent halves measure 2.34 / 2.99 vs set 2.49 (tapered ends pull the ridge median down), so the mouth is still thinned (`guddu_mouth_pair.png`); it kept 1 shape on Guddu, 0 on Raju. Guddu: stroke area 0.89, 56 face / 186 body lines, 10 solids, trace 212 ms, draft 72 ms. Raju: 0.93, 48 / 181, 5 solids, 210 ms. Suites: scene 549/549, ui 375/375, agent 41/41 |
| E3c | Third look + planner follow-up (2026-10-23) | **(a) and (c) accepted, (b) open: needs the user's decision.** `scratchpad/e3/final/{guddu,raju}_face_pair.png`. The jaw/ear/temple blobs and the jaw spur are gone; Raju is clean (ear, face and hair outlines even, brows stay solid). Stroke area: Guddu 0.89, Raju 0.93; ~0.35 s a character, 75–117 ms draft. **Guddu's open mouth still thins to two lines.** Measured per shape: mouth pieces are 1.35×/1.44× the line width by mean width (2·area/perimeter) and thick along 16–26 % of their middle, while ordinary outlines reach 1.6× and 17 %, so no width rule separates them. Tried and rejected: (1) mean width against the scope line (kept 6 Raju outlines as fills, stroke area 0.93 → 0.67); (2) a mouth area around the rig's mouth symbol (Guddu's rest mouth is drawn in `Guddu_head`'s Ink layer, its mouth symbol on the top timeline of a bone rig; the zone landed on the ear/face outline). Kept: ink inside a symbol named like a mouth (the lip-sync shapes) stays as drawn. Suites: scene 549/549, ui 375/375 (flaky `the_footer_never_overlaps_the_layer_list` failed once, passed alone and on rerun), ai_agent_coverage 41/41. |
| E1d | **Silhouette exact** (user 2026-10-23: "there shall be no pixel out of the character"): outer lines keep their outer edge; nothing pokes out | **partial** (implementer 2026-10-23): `even_lines_within(shapes, silhouette, opts)` + per-stroke `Pin` (exact-outline edge per sample, outer side pinned, ramp + 0.75-width smoothing, clamp, round ends pulled back over a ±60° fan; `set_widths` rebuilds); `KeyJob.silhouette` = filled shapes of the visible painting layers at the key. Probe `scratchpad/e3/e1d/`: paint_outside 1× = 0 / 0; silhouette_lost 1× Guddu 146, Raju 111 (as before E1d: 146 / 106). With true background (alpha) instead: 1 / 16. Rest = ink over **white fills** (hair gaps, count as background by colour) and `Raju_right_eye` whose timeline holds only the ring (both sides "outer", skin is in `Raju_head`). Hand 400 %: Guddu 14 lost / 21 outside (bracelet lobe dropped by the tracer, arm line at the bracelet), Raju 0 / 0. Stroke area 0.89 / 0.93; trace 195 / 229 ms. Suites: scene 549/549, ui 375/375, agent 41/41 |
| E1d review | Planner, 2026-10-23 | **Measure fixed, two leaks sent back.** "Out of the character" = transparency (alpha), not colour: the colour count (Guddu 146, Raju 111) is mostly lines over white fills inside the character (hair highlights), which is not a silhouette change. By alpha at 1×: Guddu 1, Raju 16. Leaks: (1) Raju's eye rings: a per-timeline silhouette doesn't see the head skin in the parent, so the ring round a see-through eye hole is centred and the hole grows (16 px at 1×, 280 at 400 % face); (2) Guddu's bracelet: the lower lobe is dropped (14 px lost) and the arm line pokes out where it meets the bracelet (21 px outside), at 400 %. |
| E1e | Composite silhouette (character in context) + bracelet; zero by alpha at 1× and 400 % | **partial** (implementer 2026-10-23): `head_rig::flatten_stack_placed` (rest flatten + every symbol placement, bone parts via `rig.posed()` as the renderer); `even_lines::context_of`/`in_context` map each scope root's rest drawing into every placement of a timeline (union with E1d's own); silhouette = fills > 50 % opaque; pins also bounded slantwise (`SIDE_DEG` 15–75°: the arm line at the bracelet crossed the edge slantwise); `kept_pieces` keeps a piece with no stroke/solid, or only both-outside strokes, as drawn. Probe `scratchpad/e3/e1e/final/` by alpha, lost/outside: Guddu 1× 22/3, face 5/4, hands 12/11 · 10/5, feet 3/0 · 2/0; Raju 1× 39/3, face 92/1 (was 310 without context), hands 1/5 · 3/0, feet 0/2 · 0/0; bracelet join 8 → 2 outside. **Not 0, why:** the renderer seam-seals fills (circle test at 4×: fill edge +0.3 device px past its geometry) but not strokes, so where ink alone bounds a see-through area (Raju's eye holes) an even stroke on the exact old edge still renders ~0.3–0.5 px inside, at any zoom; the rest are single anti-alias pixels. Stroke area 0.89 / 0.93, kept as drawn 2 / 2, trace 278 / 318 ms, draft 85 / 101 ms. Suites: scene 549/549, ui 375/375, agent 41/41 |
| E1e review | Planner, 2026-10-23 | **Accepted as far as geometry goes; the last pixels need a renderer decision.** By alpha (`scratchpad/e3/e1e/final`): the remaining flips are almost all partial anti-alias changes at the edge (median alpha change 8–147 of 255). Pixels that go fully see-through or fully painted (change > 200): Raju face 400 % 6 (eye-hole rim), Guddu left hand 400 % 2 (bracelet join), Raju 1× 1; every other crop 0. Raju's eye-hole loss fell 280 → 92 px at 400 %. Cause of the rest (implementer, 4× circle test): the renderer seals fills with a ~0.3 device-px seam stroke but not strokes, so a stroke on the exact old edge renders ~0.3–0.5 px inside a fill's edge, at any zoom. 1× counts rose (Guddu 1 → 22, Raju 16 → 39) but they are all partial-alpha edge pixels (max change 87 / 203). Stroke area 0.891 / 0.928; trace 278 / 318 ms, draft 85 / 101 ms. Suites 549 / 375 / 41 green. |
| E3 | Verify on the ep1 rigs and a fresh Ink trace (sheets, measurements) | **partial (2026-10-23).** Mouth decided by the user: "I will add the mouth shapes", so mouth ink goes in mouth symbols, which stay as drawn; no more mouth rule. Measured from `scratchpad/e3/final` renders: pixels inside the silhouette that turn background: Guddu 271 (0.061 %), Raju 183 (0.042 %); these are where the outer half of a thinner outline used to be, at the silhouette edge (target 0.05 %, Guddu just over). Dark-ink IoU before/after: 0.927 / 0.967 (target ≥ 0.8). **Not yet verified:** one undo restores the document exactly; every keyframe processed (blink/lip-sync frames); a fresh Ink + colour trace. |

## What exists (reuse, don't rewrite)

* `crates/buzz-scene/src/trace_lines.rs` `trace_core` already turns an ink
  picture into centreline strokes (Zhang-Suen skeleton, junction pairing,
  pruning) and, with `LineStyle::Stroke`, gives each line **one width** (the
  median of its distance-field width, `chain_to_shape` ~1809), or
  `uniform_width` exactly. Anything fatter than `max_line_radius` is traced as
  a filled mass (`find_masses`), not a stroke.
* `crates/buzz-scene/src/bucket.rs` `rasterise_fill(path, grid, area, ppu)`
  scan-converts a path (non-zero) into a `Grid`.
* Traced character parts keep their ink in a layer named **"Ink"** above
  "Artwork" (`crates/buzz-ai/src/character.rs` ~5780). The colour fills
  already run under the ink, so a thinner line leaves no gap.
* Face part names: `classify` in `crates/buzz-app/src/rigging/head_rig.rs:80`
  (eye, brow, lid, mouth, nose, moustache, hair, head/face).
* The renderer strokes with round joins and caps (`buzz-render/src/lib.rs`
  ~504), so a stroke already looks like a line-tool line.
* Rig Head is a full example of a command + palette + right-click + agent
  action + one undo step (`RigHead` in `buzz-ui/src/command.rs`, wired from
  `buzz-app/src/rigging.rs`).

## E1 — core (`crates/buzz-scene/src/even_lines.rs`, new, `pub mod`)

`pub fn even_lines(shapes: &[ShapeData], opts: &EvenLinesOptions) -> EvenLines`

1. **Group** the given filled ink shapes by colour (ΔE < 3); each group is
   done on its own and its strokes take that colour.
2. **Rasterise** a group into a mask with `rasterise_fill`, at a scale `ppu`
   chosen so the group's median line is ≥ 8 px wide (measure first with a
   cheap pass; cap the grid at 4096 on the long side, cap `ppu` ≤ 16).
3. **Centrelines**: build ink-on-white RGBA from the mask and run the
   existing `trace_core` path with `mode: LineArt`, `line_style: Stroke`,
   `uniform_width: None`, a fixed `ink_threshold`, `fill_enclosed: false`,
   `min_line_length` small (~2 px at the source scale), `max_line_radius` =
   `opts.solid_above` × `ppu`. Map every output path back with the inverse of
   the raster transform, divide stroke widths by `ppu`.
4. **Solid areas** (masses: hair, pupils, soles) come back as fills — keep
   them as fills, in the group colour.
5. **Straighten** (`opts.straighten` 0..1): at 0 the stroke path is the
   tracer's own; above 0, simplify the chain harder (Douglas–Peucker tolerance
   up to ~2 px at the source scale) and, from 0.5, build it from straight
   segments with the corners kept instead of Catmull-Rom — the line-tool look.
6. **Widths**: each output stroke carries its own measured width `m`. The
   caller passes a target `width` and `keep_weight`:
   * `keep_weight: false` — every stroke exactly `width`.
   * `keep_weight: true` — `width × m / median(m of the group set)`, clamped
     to 0.5×–2× `width`: a thick line stays thicker than a thin one, but each
     is even along its length.
7. Return `EvenLines { shapes (fills of masses then strokes, longest first),
   strokes, solids, measured_median }`. Also
   `pub fn measured_line_width(shapes) -> Option<f64>` (step 2's median) so the
   dialog can show defaults.

Deterministic, like the rest of the tracer (no HashMap order in output).

## E1b — fixes after the first look

1. **Masses by opening, not by component.** Mass core = ink pixels whose
   distance-field depth `r > max_line_radius`; mass region = the union of
   discs of radius `r(p)` round every core pixel (the opening, i.e. the core
   grown back by its own depth). Trace the mass region as fills (as today).
   Line remainder = ink minus the mass region, **plus** a band of
   `~0.5 × line width` inside the mass so strokes tuck under it; skeletonise
   that with no mass limit and drop chains lying wholly inside the mass. So a
   face outline that joins the hair becomes a stroke up to the hair, and the
   hair stays a fill.
2. **Which shapes are ink.** When the scope has any layer named "Ink", only
   Ink layers are processed; the dark-luma rule applies only to drawings with
   no Ink layer anywhere in scope (a plain Trace Bitmap result). Dark colour
   art (trousers, hair fills, shadows on Artwork) is never touched then.
3. **Draft preview < 300 ms**: draft scale 3 px lines, and re-trace only the
   sets whose result can change (cache per set + options).
4. **Cancel** restores without leaving a redo entry.

## E1c — fixes after the second look

* **(a) Small masses.** A mass region whose core is small (core area below
  ~(2 × line width)², or inscribed radius under 1.6 × Solid above) and
  touches lines is not a solid area: it goes back into the line remainder
  and is drawn by the strokes. Only real solids (hair, pupils, soles) stay
  fills.
* **(b) Filled shapes stay fills.** Per connected ink component (per source
  shape), when its median width is clearly thicker than the set's line width
  (> 1.8 × the median line width) it is a filled form, not a line: keep the
  original shape untouched (the mouth's inside, nostrils, a dark patch). The
  opening split still applies only to components that are lines overall.
* **(c) Spurs.** After skeletonising, prune end branches shorter than
  ~1.5 × the local line width (the jaw spur), as the tracer's own prune does
  for whiskers.

## E1d — the silhouette does not move (user, 2026-10-23)

"There shall be no pixel out of the character in the traced character."
The character's outer edge after Even Out Lines is exactly the edge before:
no pixel inside it becomes background, none outside it gets paint.

* **Silhouette** = the union of every filled shape of the same timeline at
  the keyframe's time (all layers of that symbol: Artwork, Ink, …), in the
  same space as the ink. Pass it to the core (`EvenLinesOptions` or an extra
  argument) and rasterise it with the ink.
* **Outer lines are pinned to the outer edge.** For each stroke point, if one
  side of the old line is outside the silhouette (the mask sampled at
  `point ± normal × (old_local_width/2 + 1 px)`), move the point inward by
  `(old_local_width − new_width) / 2` along the normal, so the stroke's
  outer edge lies where the old ink's outer edge was. Where the new width is
  larger than the old, the same formula moves it inward too (never outward).
  Smooth the offset along the chain (no kinks where a line leaves the edge).
* **Nothing pokes out.** Round caps and joins of pinned strokes are inside
  the silhouette by construction; any stroke end that still crosses the
  silhouette by more than 0.5 px is pulled back along its chain.
* Inner lines (both sides inside the silhouette) are unchanged: centred.
* **Check**: from 1× renders before/after, pixels whose background-ness
  changes (inside → background, or background → paint, beyond anti-alias:
  min-channel change > 64) = **0** on Guddu and Raju, and at 400 % on the
  face and hands.

## E1e — silhouette of the whole character, in context

* **Measure** "out of the character" by alpha on transparent renders: a pixel
  with alpha > 0.5 before and < 0.5 after is lost; the reverse is outside.
  Target 0 at 1× and on 400 % crops (face, both hands, feet).
* **Composite silhouette.** Flatten the scope root at rest with the same
  transforms the renderer uses (start from `head_rig::flatten_rest`, which
  already flattens an object's rest drawing with masks into its parent's
  space; bone-rig parts must be posed as the renderer poses them), and map
  that silhouette into each symbol's own space through every placement of
  the symbol (a symbol used twice — left and right eye — gets the union of
  its placements' contexts). An eye ring then sees head skin on its outer
  side and the eye hole on its inner side: pinned to the hole's edge.
* A line with **both sides outside** even in context is kept as drawn
  (no thinner line can keep both edges).
* **Bracelet**: a lobe the tracer drops as a spur or a tiny mass must come
  back (keep the original shape for any ink piece that yields no stroke and
  no solid); a stroke end that crosses the silhouette is pulled back
  (already in E1d) — find why the arm's bottom line still pokes out at the
  bracelet join and fix it.

## E2 — command **Even Out Lines…**

* **Where**: Modify ▸ Shape (next to Smooth/Optimize if present, else
  Modify), the command palette, the stage right-click on shapes/instances.
  `Command::EvenLines`, id `even_lines`.
* **Scope**: the selection (shapes, or instances → their symbols, recursively);
  nothing selected → the symbol being edited, else the whole stage. **Every
  keyframe of every layer** in those symbols (mouth shapes, blink lids, all
  poses), not frame 0 only. Shared symbols change everywhere they are used —
  that is expected (one Cast rig, one look).
* **Which shapes are ink**: filled shapes on a layer named "Ink" (any case);
  on other layers, filled shapes whose colour luma ≤ `dark` (default 0.30)
  and that come out mostly as line (if a shape yields only a solid area, leave
  it untouched). Colour fills, gradients, bitmaps, existing strokes: untouched.
* **Face or body**: a shape is a face line when its layer name, or the name
  of any symbol or layer on its path from the scope root, classifies as a face
  part (`head_rig::classify`, make it `pub(crate)`) — eyes, brows, lids,
  mouth, nose, moustache, head/face. Everything else is body.
* **Dialog** (non-modal, live preview on the stage while open — preview is
  exactly what Apply commits; Cancel restores):
  * **Face lines** width slider, px (0.25–12, default = measured median of the
    face ink, rounded to 0.25)
  * **Body lines** width slider, px (default = measured median of the body ink)
  * **Keep thick and thin** checkbox (default on)
  * **Straighten** slider 0–1 (default 0)
  * **Solid above** slider, px (default = the tracer's `max_line_radius`
    equivalent): thicker ink than this stays a filled area
  * A line under the sliders: "N face lines, M body lines, K solid areas kept".
  Preview may compute at lower raster scale while a slider drags, full scale on
  release; Apply always full scale.
* **Apply**: one undo step "Even Out Lines". The new strokes replace the ink
  shapes in place (same layer, same paint order slot as the first replaced
  shape). After that each line is an ordinary stroke: Properties ▸ stroke
  width edits it like any line-tool line.
* **Agent**: `even_lines {element?, face_width?, body_width?, keep_weight?,
  straighten?, solid_above?} -> {face_lines, body_lines, solids, face_width,
  body_width}` (AI must be able to do what the user can — AI_AGENT_PLAN).
  Add it to the agent coverage list like `rig_head`.

## E3 — verify (the planner does this; images looked at)

On `B:\youtubeProjects\Originals\series\ep1\buzz\rigs\Guddu.buzz`,
`Raju.buzz`, and a fresh Ink + colour trace of one ep1 T-pose PNG:

* Sheets before/after at fit and at 400 % on the face (scratchpad `e3\`).
* Every replaced line is a stroke; widths: face lines = face slider, body
  lines = body slider (keep_weight off), within the 0.5–2× band (on).
* **No gaps**: pixels inside the character's silhouette that are background
  after but not before = 0 (beyond anti-alias, ≤ 0.05 %).
* Ink coverage IoU before/after ≥ 0.8 at keep_weight on and widths = measured.
* Mouth-shape symbols and eyelid frames processed on every keyframe.
* One undo restores the document exactly.
* Time for a whole character (target < 2 s on Guddu; preview drag < 200 ms).
* Suites stay green: buzz-scene lib, buzz-ui lib (command count +1),
  buzz-app ai_agent_coverage. No new tests (user rule 2026-10-11).

## Constraints

* Don't change what Trace Bitmap itself produces; this runs after it.
* No `cargo fmt` (repo isn't rustfmt-clean).
* Never real mouse/keyboard input to verify; headless builds only.
