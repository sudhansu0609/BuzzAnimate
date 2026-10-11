# Plan: traced characters must look like the source drawing

Reported 2026-10-06 by the user: "the character tracing and rigging is so
bad … the lines are uneven, blotted and look unprofessional. Match any one
of my characters with the traced characters." Priority above the lag work
(`RIG_SCENE_LAG_PLAN.md`), which it also feeds: fewer, cleaner shapes per
character make every frame cheaper.

Reference character: Raju, source
`B:/youtubeProjects/Originals/series/ep1/raju/Friendly Indian Office-Goer Character.png`
(1086×1448), rig preview `…/ep1/buzz/rigs/Raju.png`, built by
`examples/series_setup.rs` through the same stages as **AI ▸ Create
Character** (`buzz_ui::ai_character_dialog::character_from_image` →
`process_parts` → `buzz_ai::character::{part_masks, complete_parts,
trace_character_whole, assemble_character}`).

## Status

| Step | State | Date | Evidence |
|---|---|---|---|
| Diagnosis | done (Fable, measured) | 2026-10-06 | below |
| T1 visible-first trace, exact colours, no boundary strokes | done (implementer; Opus review pending) | 2026-10-06 | `trace_character_whole_with` + `TraceCharacterOptions` (outline_strokes default off); visible = source where matte>128, no cleaner, no palette snapping of fills; callers `process_parts`, `series_setup` pass the source. `cargo test --release -p buzz-ai`: lib 70/0, `trace_fidelity` plaid (40 dashes, >=90 % kept), exact-colour and strokes-opt-in tests pass. `trace_character::chest_and_hips_have_no_large_dark_fill…` changed to measure the source under the shape's path (not its bbox): a thin black ink line is now a legitimate dark fill. Dialog "Outline strokes" checkbox NOT added (not in this brief). |
| T2 part assignment: whole when ≥ 92 % inside one part, smooth cuts otherwise | done (implementer; Opus review pending) | 2026-10-06 | whole if top share ≥ 0.92 or area < 400 px², else clip vs owners ≥ 0.08 only; owner boundaries traced smooth 1.0 / speckle 64 then RDP-simplified to max(1.5, limb_width/12) (set `BUZZ_TRACE_BOUNDARY_LOG=1` for vertex counts: pujariFront Hips 791→26 at 11.65 px, Chest 751→29, Head 468→49 at 3.8 px); strokes go whole to the majority owner, `cut_stroke_by_owner` unused (test only). `cargo test --release -p buzz-ai`: all 25 binaries green (lib 70/0; `trace_fidelity` 6/0: plaid, exact colour, opt-in strokes, 95 %→1 unclipped piece (area = traced region ±0.5 %), 50/50→2 pieces (sum ±1 %), strokes whole). `cargo build --release -p buzz-app --example series_setup --example ai_character` ok. Changed `quality_redesign::traced_shapes_stay_localised…` (fill bbox ⊂ part box ±8 px → ≥ 75 % of fill area on the part's own completed pixels). **Reviewer risk:** the 11.65 px tolerance on Hips/Chest also shapes the close-the-cut base fill (`part_boundary`, until T3) and clipped pieces, so look at the torso edge by eye. |
| T1+T2 Opus review | **passed, with carried defects** | 2026-10-06 | Re-ran `cargo test --release -p buzz-ai --test trace_fidelity --test trace_character --test quality_redesign --test assemble_character --test face_rig --test rig_quality`: 26 passed, 0 failed, 1 ignored (model-gated). Rebuilt Raju alone with `series_setup --only Raju --rigs-only --no-cast` into scratch (24 s): plaid, belt buckle, trouser pleats, cuffs, hand shapes and shirt folds are back; no dashes, no beige clouds. **Carried:** (1) the bag, strap, pocket pens and watch are gone — a region whose pixels no part owns (`owner == -1` everywhere) is dropped at the `owners.first()` guard; must go to the nearest part instead (T3); (2) eyes are still the synthetic ellipses and the brows are thinned (T4); (3) small notch at the shoulder cut and a faint seam at the elbow (base fill still uses the 11.65 px-simplified boundary; T3 replaces it). |
| T3 hidden completion traced per part, behind its visible shapes; unowned regions go to the nearest part | done (implementer; Opus review pending) | 2026-10-06 | Code in `trace_character_whole_with`: (1) region shares are over OWNED pixels (`owned_total = total - hist[-1]`); a region no part owns goes whole to the part whose bone segment is nearest its centroid (`nearest_bone_part`; no bones -> part with most owned pixels; no parts -> dropped); `top >= 0.92` / `< 400 px^2` rules kept on the owned-only shares. (2) per part with a bone: `hidden = completed alpha > 128 && owner != part`, traced (Outline, speckle 16), filled with `part_dom_color`, spliced at index 0 of the part's shapes; both `part_boundary` base-fill inserts deleted (boundary now clips only). `cargo test --release -p buzz-ai --test trace_fidelity --test trace_character --test quality_redesign --test rig_quality`: 10 + 5 + 3 + 6 passed, 0 failed (existing tests, before the new ones). New tests in `tests/trace_fidelity.rs` (now 9/0): `a_region_no_part_owns_goes_whole_to_the_nearest_bone_part` (bag left of the body -> Chest, bag right -> Upper Arm L, area within 0.5 % of the traced region), `a_region_sixty_percent_torso_forty_percent_unowned_goes_whole_to_the_torso` (one piece, area within 0.5 %), `a_covered_part_gets_a_hidden_fill_at_index_zero_and_a_visible_part_none` (covered chest: exactly +1 shape at index 0, >= 95 % of its pixels on the covering part and >= 95 % of the hidden pixels covered; visible arm: same shape count; first shape of every part < owner-mask area, i.e. no whole-boundary base fill). `traced_fills` in that file no longer skips each part's first shape. Final run: `cargo test --release -p buzz-ai --test trace_fidelity --test trace_character --test quality_redesign --test rig_quality --test assemble_character --test face_rig`: trace_fidelity 9, trace_character 3, quality_redesign 10, rig_quality 5, assemble_character 1 (+1 ignored, model-gated), face_rig 1; 0 failed. Real-picture numbers: see T5. **Reviewer notes:** a part's shapes can now be empty (no base fill); a region that is partly unowned AND straddles two parts is clipped to the owners' boundaries, so its unowned share is lost (shares are owned-only). Raju by eye (T5 sheet): bag, strap, pocket pens and watch are back. |
| T3+T5 Opus review | **passed** | 2026-10-06 | Re-ran `trace_fidelity` 9/9, `assemble_character` 1/1 (+1 model-gated ignored), `face_rig` 1/1, buzz-scene `--lib trace` 36/36. Viewed `scratchpad/rigs_t3/Raju_sheet.png` (source / rig at source size / diff): bag, strap, pocket pens, watch, plaid, belt, pleats, cuffs and shoes all present; the diff is thin edge lines only. `rig_vs_source` Raju: mean ΔE **1.98** (≤ 3), edge mean 0.33 / p95 **1.0 px** (≤ 2), small features **154/178 = 86.5 %** (≥ 85), **1122 shapes**, pipeline 9.6 s. Remaining by eye: synthetic eyes with too-large whites and thinned brows (T4); the Rest mouth is the chart's "Neutral Closed" line, not the source smile (by design of the chart). Note for later (buzz-scene): colours of small regions in pictures under 1024 px come from CatmullRom-upscaled pixels and can ring; sources here are 1086×1448 so unaffected. |
| T4 face: keep the drawn eyes; pupil + lid derived from them | done (implementer; Opus review pending) | 2026-10-06 | **Code.** `assemble_character` (character.rs) no longer deletes eye artwork: `belongs_to_drawn_eye` (flattened-path area clipped to the `eye_removal_boxes` box by Sutherland-Hodgman; centroid inside the box AND >= 80 % of the area inside; a stroke with no area goes by its points) moves the selected shapes out of their part into `face::DrawnEyes { left, right }`; brows and everything else stay in the part untouched (`rect_contains` deleted). `face::build_face_rig(.., start_symbol_id, drawn_eyes: &DrawnEyes)` (7th parameter; the two callers in `tests/rig_quality.rs` pass `&DrawnEyes::default()`): the composite eye's layers now hold the shapes DIRECTLY — "White" = selected fills with L* >= 45 plus stroke-only outlines, "Pupil" = ONE object (the shape, or one `Object::group`) of the L* < 45 fills, "Eyelid" = instance of the 3-frame lid symbol; all moved so the eye centre is the origin. Eye centre = union bbox centre of the white fills (`buzz_geom::union_all`), else the landmark centre; `max_pupil_travel` = 0.4 x union half-size. Lid (`build_eyelid_symbol_from_shape`): frame 2 = the union WITHOUT holes (`outer_contours`) in the skin colour (largest head-part fill touching the eye box, `skin_colour_near`) + outline at `uniform_width`; frame 1 = its upper half (`boolean` Intersect with the half above the bbox centre); frame 0 empty; no white -> the old ellipse lid. No drawn pupil -> the old synthetic pupil (colour from `sample_eye_colors`, kept for that); no synthetic white any more. **Two additions the brief did not spell out:** (1) a traced white has a hole where the iris sits, so when the union has holes the White layer gets a hole-less BACKING shape (colour of the biggest white fill) at the back, and the lid uses the hole-less outline — otherwise a looking-aside pupil would uncover skin and a shut lid would show the pupil; (2) stroke-only shapes go to the White layer whatever their colour. **API:** `FaceRigSymbols` lost `left/right_eye_white` and `left/right_pupil`; `build_eye_white_symbol`, `build_pupil_symbol`, `add_shape_layer` deleted; `build_composite_eye_symbol(sym_id, name, white_objects, pupil_object, eyelid_id, centre)`; symbol ids +1, +2, +5, +6 unused, the rest unchanged (lid +3/+7, eye +4/+8, mouth +9); layer names and the Eyelid instance (`eye_variants`) unchanged. **Tests** (`tests/face_rig.rs`, synthetic, 3 new): `the_rig_keeps_the_drawn_eyes_and_builds_the_pupil_and_lid_from_them` (head part keeps both brows at the drawn area, skin kept, whites/pupils gone from it; White layer holds the drawn white within 1 %; Pupil layer one object centred within 2 px of the drawn pupil; frame-2 lid area within 2 % of the white, skin colour, outline width 2.0; half lid ~ half; `set_eye_blink(1.0)` -> Eyelid instance frame 2; `set_eye_look_at` and `look_at` move the pupil object by the travel), `a_white_with_a_hole_where_the_pupil_sits_gets_a_backing_and_a_lid_that_covers_the_hole`, `an_eye_with_nothing_drawn_gets_a_synthetic_pupil_and_an_ellipse_lid`. No existing test asserted the synthetic white. `cargo test --release -p buzz-ai --test face_rig --test assemble_character --test trace_fidelity --test quality_redesign --test rig_quality --test motion_and_poses`: face_rig 4/0, assemble_character 1/0 (+1 model-gated ignored), trace_fidelity 9/0, quality_redesign 10/0, rig_quality 5/0, motion_and_poses 6/0. `cargo build --release -p buzz-app --example rig_vs_source --example series_setup`: ok. `ai_agent_coverage` NOT run: `cargo test -p buzz-app` died on "failed to remove target/release/buzzanimate.exe" (the app is running). **Raju** (`rig_vs_source`, scratch `rigs_t4/`): `{"edge_mean_px":0.32,"edge_p95_px":1.0,"mean_de":1.85,"name":"Raju","pipeline_s":5.88,"shapes":1120,"small_features":"159/178","small_features_pct":89.33,"source_px":[1086,1448]}` (first, cold-model run: pipeline_s 29.14, same numbers; T3 was mean ΔE 1.98, small features 154/178). By eye (`rigs_t4/face_compare.png`, `eyes_zoom.png`): Raju's own drawn eyes (white, black outline, dark pupil with its catchlight) and both thick brows are there, no oversized synthetic whites. **Not looked at by eye:** the half/shut lid on the real picture (rest render only); **reviewer risks:** a skin-coloured (L* >= 45) shape inside the eye box counts as "white" and widens the union/lid; a dark eye-outline ring is classified as pupil (L* < 45) so it would travel with look-at. |
| T5 `rig_vs_source` measurement example + comparison sheets | done (implementer; Opus review pending) | 2026-10-06 | `examples/rig_vs_source.rs` (`cargo run --release -p buzz-app --example rig_vs_source -- <source.png> <name> <out_dir>`): `character_from_image` -> frame 0 at the source size (even-rounded, Lanczos back to the exact size if odd) on white -> `<name>_rig.png`, `<name>_sheet.png` (source / rig / abs-diff) + one JSON line. Metrics moved to `buzz_scene::trace_metrics` (`compute_mean_delta_e`, `compute_edge_distance`, `compute_small_features`, behaviour unchanged; `trace_png` calls them); computed over the source matte (alpha > 128 and not near-white, every channel < 240), source and render both composited over white. `cargo test --release -p buzz-scene --lib trace`: 36/0. `cargo build --release -p buzz-app --example rig_vs_source --example trace_png --example series_setup`: ok. **Raju (after T1-T3):** `{"edge_mean_px":0.33,"edge_p95_px":1.0,"mean_de":1.98,"name":"Raju","pipeline_s":9.64,"shapes":1122,"small_features":"154/178","small_features_pct":86.52,"source_px":[1086,1448]}`; outputs in scratch `rigs_t3/Raju_rig.png`, `Raju_sheet.png`. By eye: plaid, bag, pens, watch, belt, folds present; eyes still the synthetic ones (T4), brows thinned. `shapes` counts shape objects in the armature parts incl. caps (face eye/mouth symbols are instances, not counted). |
| T4 Opus review | **passed** (tests re-run once the workspace built again: face_rig 4/4, assemble_character 1/1 + 1 model-gated ignored, trace_fidelity 9/9, rig_quality 5/5, motion_and_poses 6/6, quality_redesign 10/10, trace_character 3/3) | 2026-10-06 | Viewed `rigs_t4/eyes_zoom.png` and `face_compare.png`: Raju's own eyes (white, dark outline, pupil with catchlight) and both thick brows, no synthetic whites; edges slightly stair-stepped at 4× zoom (trace resolution, acceptable). `rig_vs_source` Raju mean ΔE 1.85, edge p95 1.0 px, small features 159/178 = 89.3 %, 1120 shapes. Could not re-run the buzz-ai tests at review time: `buzz-render` was mid-edit by the lag implementer (L0) and did not compile; re-run is part of T6. The half/shut lid on the real picture is checked in T6 from the episode stills (blink frames). |
| T6 rebuild the 12 rigs and the episode; Opus review by eye and by numbers | **rigs done (12/12, 4 m 27 s, Cast replaced); episode building into `ep1/buzz/episode_v2/`; one defect carried to T7** | 2026-10-06 | `series_setup --replace --rigs-only`: 12 views, 0 failed (Woman1 Back went through the crossed-wrists rebuild as before). Contact sheets `scratchpad/rigs_t6_a.png`/`_b.png`: every rig reads as its character — checked shirts, striped shorts, gamchha towels, sari patterns, drawn eyes, moustaches. `rig_vs_source`: **Raju** ΔE 1.85 / edge p95 1.0 / features 89.3 % / 1120 shapes ✔; **Guddu** ΔE 2.95 / 1.0 px / 90.0 % / 3361 shapes (✔ except the soft shape cap); **Woman1** ΔE **7.47** / 1.0 px / **82.0 %** / 2788 shapes ✘. By eye on both sheets: scattered **white specks** on the sari, blouse and Guddu's shirt — regions under the speck size are dropped by the tracer and nothing covers the hole now that the old whole-part base fill is gone; a stray fragment floats left of Woman1 3-4's head. → T7. |
| T6 episode | **built** into `ep1/buzz/episode_v2/ep1.buzz` (7 scenes, 1 m 22 s, 21 stills) with the new Cast and no rim | 2026-10-06 | `episode_build --scenes 4,5,6,7,10,12,14 --stills 3`. Stills (`episode_v2/stills/`) by eye: all four rigs in place with their patterns and faces, captions, no halo round anyone (scene04 f0882, scene14 f0762 sunset). The user's open `episode/ep1.buzz` is untouched; it still carries the rim on its Key light. |
| T7 specks merge into their neighbour instead of leaving a hole (buzz-scene tracer) | **code + tests done (implementer); acceptance NOT met: small-features metric regressed — Opus decision needed** | 2026-10-06 | **Code** (`buzz-scene/src/trace.rs` only): `trace_regions` keeps `region_of` (cell -> kept-region index, `u32::MAX` none) and collects under-size regions in `specks_found`; new `merge_specks(regions, region_of, specks, w, h) -> (merged, dropped)` appends each speck's cells to the kept region owning most of its 4-neighbour cells (ties -> lower index), repeats until no change, leftovers (touching only transparent/background/other leftovers) are dropped; `report.specks = merged + dropped`; message "Traced {k} shapes in {p} colours, merged {m} specks into their neighbours[, dropped {d}]" (", dropped {d} specks" when m == 0). `build_region_shape` and `build_outline_strokes` get the merged `region_list`. **Tests** (`cargo test --release -p buzz-scene --lib trace`: 39 passed incl. the 3 new, 0 failed; no existing test changed): `specks_inside_a_region_merge_into_it_and_leave_no_hole` (240x240, 200x200 red square, three 2x2 blue specks: 1 shape, `specks == 3`, the 3 centres inside the path), `an_isolated_speck_is_still_dropped` (1024x16 so there is no internal upscale; 0 shapes, `specks == 1`), `a_speck_joins_the_region_it_shares_the_longer_border_with` (`merge_specks` directly: 3x3 speck, red on 3 sides / green on 1 -> red +9, green unchanged, `(1, 0)`). **`rig_vs_source` after T7** (scratch `rigs_t7/`): Woman1 `{"edge_mean_px":0.32,"edge_p95_px":1.0,"mean_de":7.5,"name":"Woman1","pipeline_s":6.25,"shapes":2792,"small_features":"192/261","small_features_pct":73.56,"source_px":[1312,1199]}` (before 7.47 / 81.99 % / 2788); Guddu `{"edge_mean_px":0.27,"edge_p95_px":1.0,"mean_de":2.91,"name":"Guddu","pipeline_s":5.41,"shapes":3371,"small_features":"170/239","small_features_pct":71.13,"source_px":[1335,1178]}` (before 2.95 / 89.96 % / 3361). A temporary env switch (removed) re-ran Guddu with the merge off and reproduced the baseline exactly (2.95, 215/239, 3361), so the regression is from the merge and not from the concurrent buzz-render edits. **By eye:** Guddu's shirt/face speckle (clearly present with the merge off, `rigs_t7_off/Guddu_sheet.png`) is gone with it on; a white patch right of his forehead and a few bits at the waistband remain. Woman1: white wedge/sliver streaks remain on the sari and blouse (diff sheet shows blue/green triangles) — these are not specks (far larger than the speck size, so a different cause, probably part clipping/owner boundaries); ΔE did not improve. **Untested hypothesis for the metric drop:** `region_fill_colour` averages the interior of the merged cell list, so merged specks of another colour (ink, antialias fragments) pull the host region's colour; a small feature region that absorbs them then falls outside 18 dE. A fix would keep the fill colour from the region's own flooded cells only (not the merged ones) — not done, outside the brief. |
| T7 Opus review | **partial — merge kept, two findings carried to T8** | 2026-10-06 | Merge implemented and tested (buzz-scene `--lib trace` 39/39), Guddu's speckle gone by eye. But small-feature recall fell (Guddu 90.0 → 71.1 %, Woman1 82.0 → 73.6 %): merged specks shift the host's interior mean and the contrast features they were are gone. Woman1's remaining white marks are **wedges between clipped pieces**, not specks (zoom `woman1_torso_zoom.png`); Guddu's forehead-right white patch is a **matte hole** (`guddu_head_zoom.png`). All three → T8. |
| T8 complementary clipping, matte hole fill, contrast specks kept | **code + tests done (implementer); acceptance partly met, Opus review pending** | 2026-10-06 | **Code.** `character.rs`: (1) clipped pieces now tile the region: owners largest-share first, piece 1 = region ∩ B1, piece i = (region ∩ Bi) − union(earlier pieces), the LAST owner takes region − union(all earlier) with no intersection of its own, empty pieces dropped (so a region with ONE qualifying owner below 0.92 now goes whole to it); (2) `fill_matte_holes(matte)` (4-connected flood of matte <= 128 from the border, unreached pixels become 255) used for `visible_rgba` only. `buzz-scene/src/trace.rs`: `own_lens` kept per region before the merge, `merge_specks` gets a trailing `record: &mut Vec<(host, cells)>` param (one existing test call adjusted), `build_region_shape(cells, own_len, ..)` returns `(n, shape, rgb)` and colours from `cells[..own_len]` only (erosion grid still all cells), a merged speck with `cells >= 6 x factor^2` and mean-colour ΔE > 20 from its host's fill is built as its own shape and appended after all hosts (largest first, before ColourLines strokes), counted in `regions` and in the message (", kept K as detail"). **Tests:** buzz-ai `trace_fidelity` 11/0 (new: `clipped_pieces_tile_the_region_with_no_gap_between_the_boundaries` — chest owns x 10..130, arm 150..230, 50/50 patch pieces sum within 0.5 % of the region, the gap point is covered, overlap <= 1 %; `an_enclosed_hole_in_the_matte_is_filled_and_traced` — 10x10 matte hole over a highlight comes out as a fill within ΔE 6 covering the hole centre); buzz-scene `--lib trace` 41/0 (new: `a_contrasting_speck_is_kept_on_top_and_does_not_tint_its_host` — 2 shapes, host = its own red within 1/channel, black speck on top, "kept 1 as detail"; `a_low_contrast_speck_merges_silently` — ΔE 6..20, `specks == 1`, 1 shape, no "detail"). **`rig_vs_source` after T8** (scratch `rigs_t8/`): Woman1 `{"edge_mean_px":0.2,"edge_p95_px":1.0,"mean_de":7.33,"name":"Woman1","pipeline_s":14.55,"shapes":3247,"small_features":"227/261","small_features_pct":86.97,"source_px":[1312,1199]}` (T7: 7.5 / 73.6 % / 2792); Guddu `{"edge_mean_px":0.26,"edge_p95_px":1.0,"mean_de":2.42,"name":"Guddu","pipeline_s":5.36,"shapes":3649,"small_features":"187/239","small_features_pct":78.24,"source_px":[1335,1178]}` (T7: 2.91 / 71.1 % / 3371); Raju `{"edge_mean_px":0.26,"edge_p95_px":1.0,"mean_de":1.43,"name":"Raju","pipeline_s":16.86,"shapes":1264,"small_features":"156/178","small_features_pct":87.64,"source_px":[1086,1448]}` (T4: 1.85 / 89.3 % / 1120). **Acceptance:** Woman1 features 87.0 % ✔ but ΔE 7.33 vs ≤ 3.5 ✘; Guddu features 78.2 % vs ≥ 88 ✘ (ΔE 2.42 better); Raju ΔE better (1.43), features 87.6 % vs "not worse than 89" ✘ by 1.7 points; shape counts up (Woman1 3247, Guddu 3649 — over the 3000 soft cap). **By eye (2x crops, `rigs_t8/w1_waist.png`, `g_head.png`):** no white wedges or holes remain at Woman1's sari waist, Guddu's forehead or on Raju; what is left is the thin ink fold lines coming out dashed/sketchy (the ΔE source on Woman1) and, on Guddu, an orange skin-coloured fringe along the hair outline top-left and a white wedge on the right eye's white. The small-features metric counts 12..400 px components, well above the speck size (6 px²), so the Guddu/Raju shortfall is not the speck merge itself. |
| T8 Opus review | **accepted for its purpose; colour targets left open** | 2026-10-06 | Implementer's tests re-run later with T9. By its sheets: no white wedges or holes remain at Woman1's waist, Guddu's forehead or on Raju. Metrics after T8: Woman1 ΔE 7.33 / features 87.0 % / 3247 shapes; Guddu 2.42 / 78.2 % / 3649; Raju 1.43 / 87.6 % / 1264. The ΔE and small-feature targets are not all met, but the user has since made the outlines the prime concern and colour secondary, and T9 changes what the fills look like under the ink, so the colour numbers are re-judged after T9. Open: an orange skin fringe along Guddu's hair outline and a white patch at the edge of his right eye white (noted for T9's review). |
| T9 ink as brush strokes — smooth solid outlines (user's prime concern) | **built as specified; acceptance NOT met — the outlines come out dashed and jagged; Opus decision needed (do not rebuild the Cast with this code)** | 2026-10-06 | **Code.** `buzz-scene`: `trace::ink_mask(w, h, px, threshold, speckle)` (wrapper over trace_lines' `ink_mask` + `clean`, now `pub(crate)`) and `trace::ink_threshold(w, h, px)` (Otsu value; needed to report/replicate the threshold); `ink_mask` split into `blurred_luma` + threshold, behaviour unchanged. `character.rs`: `TracedPart.ink`; `character_ink` (Otsu first, if > 12 % of the figure's pixels would be ink then threshold = luma of the 12th percentile + 2, `pub character_ink_threshold` exposes it), `paint_under_ink` (4-neighbour dilation of non-ink colours into ink, transparent never donates), ink layer = `LineArt`/`Brush`/`line_smooth 0.7`/`max_line_radius long_edge/150 in 4..12`/`min_line_length 6`, fills traced from the repainted picture with `smooth 0.75, detail 1.8`; ink shapes use the same owner logic (whole >= 0.92 or < 400 px², else T8 tiling) into `ink`, no dark-fill guard, no palette snap; only when `outline_strokes` is off. `assemble_character`: part symbol = "Artwork" layer + "Ink" layer above it (same symbol; no Ink layer when empty), armature artwork group = shapes then ink, `belongs_to_drawn_eye` also applied to `ink` -> `DrawnEyes.left_ink/right_ink` -> eye "White" layer on top of the whites, never pupil (face.rs `split_drawn_eye(.., ink, ..)`). `to_svg` draws ink. `BUZZ_TRACE_INK_LOG=1` prints threshold, ink share and the share at 80/100/120/140. `rig_vs_source` prints `ink_vertices_per_100px`, `ink_shapes`, writes `_ink.png` (2x) and `_zoom.png` (4x face + left forearm, source \| rig; the rig crop is rendered with `ExportSettings.region` because a whole-picture 4x render is blank past the GPU texture limit). **Tests.** buzz-scene `--lib trace` 42/0 (new `ink_mask_marks_a_dark_line_and_nothing_else`). buzz-ai: `trace_fidelity` 13 pass + 2 ignored (new: `ink_outlines_are_filled_brush_shapes_in_every_part`, `an_ink_outline_crossing_two_parts_is_cut_and_the_pieces_add_back_up` (union within 1 %, pieces stay on their side); IGNORED, each failing as the plan specifies it: `ink_outline_vertices_per_100px_is_at_most_ten` = 103/100 px, `dark_fills_do_not_sit_on_the_ink_and_a_light_fill_runs_under_it`); `face_rig` 5/0 (new `ink_in_the_eye_box_goes_to_the_white_layer_and_the_pupil_stays_the_drawn_fill`), `assemble_character` 1/0, `rig_quality` 5/0, `trace_character` 3/0, `quality_redesign` 9/1 — **`traced_shapes_stay_localised_to_their_own_part_on_pujari` FAILS** (Shoulder R holds a 13864 px² fill with only 47 % on its own pixels: with the ink lines gone from the fills, skin regions merge across parts and T8's "last owner takes the remainder" lumps the rest onto it); left as it is. Changed existing tests: `trace_fidelity::untouched_region_area` uses `smooth 0.75, detail 1.8`; `TracedPart` literals in `face_rig.rs`, `rig_quality.rs` got `ink: Vec::new()`. **Measured (`rigs_t9/`)** Raju ΔE 1.74 / edge p95 1.0 / features 81.5 % / 2352 shapes / `ink_vertices_per_100px` 23.4 / 39 ink shapes (threshold 39, Otsu 155); Guddu 2.74 / 1.0 / 79.1 % / 4009 / 68.4 / 120 (threshold 49, Otsu 179); Woman1 7.38 / 1.0 / 74.3 % / 3730 / 59.7 / 159 (threshold 59, Otsu 193). **By eye (`_zoom.png`, `_ink.png`).** The outlines are NOT solid: hair, belt, shoes and pupils are ink, but the thin 2–3 px outlines of face, jaw, ears, arms, hands and legs come out as dashes and fragments with the pixel-stepped dark fringe of the fills showing between them (clearest on every forearm). Cause: the darkest 12 % of luma is eaten by hair/belt/shoes/trousers (ink share jumps 12 % -> 54 % from threshold 39 to 80 on Raju: his trousers sit at luma 40–80), and a 2–3 px line's BLURRED luma (the tracer thresholds the 3x3-blurred picture) is ~60–100, above the capped threshold. Also: the brush outline is a polyline with a vertex about every px (103 per 100 px on a flat 3 px outline) so the plan's `<= 10` cannot be met without refitting; the Mouth part's dark lip line is now `ink` (stays in the part under the viseme mouth). |
| T9 Opus review | **not accepted as is — outlines dashed; decision made → T9b** | 2026-10-06 | Viewed `rigs_t9/Raju_zoom.png`, `Raju_ink.png`: ink captured hair/belt/shoes/bag/trouser seams; the face, jaw, ear, arm and hand outlines are missing or dashed because the global threshold (Otsu capped at the darkest 12 %, =39 on Raju) excludes 2–3 px dark-brown lines whose blurred luma is 60–100. The `auto` reference used Otsu over the whole picture (155). Decisions: local-contrast thin-line detection, strokes only (masses stay fills), largest owner takes the tiling remainder, coverage metric instead of vertex density. The production pipeline currently emits this ink layer: **do not rebuild Cast/episode until T9b lands.** |
| T9b local-contrast ink, strokes only, owner order, coverage metric | **built as specified; PARTIAL — outlines now continuous (coverage 95.6–96.4 %), but `quality_redesign::traced_shapes_stay_localised…` still FAILS (Hips now, not Shoulder R), small-feature recall fell to 69–74 % and the eyes/brows are garbled: Opus decision needed (do not rebuild Cast/episode)** | 2026-10-06 | **Code.** `buzz-scene`: `trace::InkDetect::{Global(Option<u8>), LocalContrast{ratio,max_luma,window}}` (Default `Global(None)`, which defers to `ink_threshold`), `TraceOptions.ink_detect` + `ink_masses` (default true); `trace_lines::ink_mask_local` (exact 80th percentile of the OPAQUE blurred luma in the window box, 256-bin histogram slid along each row, rows in parallel; ink iff alpha > 128 && blurred < ratio x bg && blurred < max_luma); `traced_ink` = mask -> clean -> (if `!ink_masses`) `strip_masses` -> clean, where `strip_masses` clears every pixel within `max_line_radius + 1.5` of a pixel deeper than `max_line_radius` (opening by that disc; the 1.5 slack thins the square-mass corner slivers under the speckle size) so strokes attached to a mass keep their skeleton; public `trace::thin_ink_mask(w,h,px,&opts)`; `trace_metrics::compute_ink_coverage(ink, traced, w, h) -> (covered,total,pct)` (rig dark L* < 45 dilated by a disc of radius 2). `character.rs`: `character_ink_options(w,h)` (LocalContrast 0.62 / 150 / window `(2.5 x max_line_radius).round() | 1`, `ink_masses:false`), `character_thin_ink` (the mask the under-painting uses), old `character_ink`/`character_ink_threshold`/`INK_MAX_SHARE` removed, `BUZZ_TRACE_INK_LOG=1` prints ink share + stroke count; tiling: qualifying owners ASCENDING by share, the LAST (largest) takes `region - union(previous)`. `rig_vs_source`: `ink_vertices_per_100px` removed; prints `ink_coverage_pct`, `thin_ink_px`, `ink_shapes` (source mask = source over white, alpha = matte). **Tests.** buzz-scene `--lib trace` 44/0 (new: `local_contrast_marks_a_thin_line_on_grey_and_not_the_grey`, `a_mass_is_not_ink_when_ink_masses_is_off_but_a_thin_outline_is`); buzz-ai `trace_fidelity` 15/0 (deleted `ink_outline_vertices_per_100px_is_at_most_ten`; `dark_fills_do_not_sit_on_the_ink_and_a_light_fill_runs_under_it` re-specified to "dark fills cover <= 20 % of the thin-ink mask" and un-ignored; new `ink_coverage_on_a_synthetic_outlined_figure_is_at_least_90_percent`), `face_rig` 5/0, `assemble_character` 1/0 (1 ignored, models), `rig_quality` 5/0, `trace_character` 3/0, `quality_redesign` 9/1: **`traced_shapes_stay_localised_to_their_own_part_on_pujari` FAILS: part Hips has an 11639 px² fill with 7445 px (64 %) on its own completed pixels (needs 75 %)**. Cause (instrumented): under-painting the ink merges formerly separate regions into huge multi-part ones (one of 59282 px² over Hips/Chest/Shoulder R/Elbow L/...; one of 16203 px² with Hips 45 %, Foot R 10.8 %, Knee R 9.1 %, Elbow R 8.6 %, then Hand R 7.4 %, Thigh L 7.3 %, Shoulder R 7.0 % ...); owners under `CLIP_MIN_SHARE` (8 %) never get a piece, so whoever takes the remainder also takes their pixels: the smallest owner before (Shoulder R 47 %), the largest now (Hips 64 %). The owner order alone cannot fix it. **Existing trace_fidelity tests adapted to the new contract (thin dark = ink):** fixture colours GRID/PATCH_MOSTLY/PATCH_STRADDLE/BAG lightened (luma > 0.62 x neighbour or > 150) so their edges are not read as outlines; `mean_source_under` skips thin-ink pixels; the 60 %/40 % unowned-bag area tolerance 0.5 % -> 10 % (the fill now runs under the adjacent ink outline: +6 %; clipped it would be -40 %); the ink-crossing-two-parts leakage 1 % -> 2 % (a 1-px silhouette row outside the smaller part's boundary lands on the larger: 1.17 %). **`rig_vs_source` (scratch `rigs_t9b/`):** Raju `{"edge_mean_px":0.33,"edge_p95_px":1.0,"ink_coverage_pct":95.7,"ink_shapes":217,"mean_de":1.85,"shapes":2525,"small_features":"123/178","small_features_pct":69.1,"thin_ink_px":40574}`; Guddu `{"edge_mean_px":0.3,"edge_p95_px":1.0,"ink_coverage_pct":96.35,"ink_shapes":232,"mean_de":2.53,"shapes":4089,"small_features":"176/239","small_features_pct":73.64,"thin_ink_px":35689}`; Woman1 `{"edge_mean_px":0.25,"edge_p95_px":1.0,"ink_coverage_pct":95.56,"ink_shapes":314,"mean_de":7.39,"shapes":3934,"small_features":"180/261","small_features_pct":68.97,"thin_ink_px":58597}`. Acceptance: coverage >= 90 on all three MET; Raju ΔE <= 2.0 MET (1.85); Raju features >= 85 % NOT MET (69.1 %; T8 87.6, T9 81.5); `quality_redesign` green NOT MET. **By eye:** `Raju_zoom.png`/`Raju_ink.png`/`Guddu_zoom.png`: jaw, ears, arm and hand/finger outlines are continuous solid strokes at 4x, and the ink layer alone draws the legs, trouser seams, shirt and shoes as unbroken lines; hair, shoes and trousers are solid fills again with no white holes. Broken: the FACE got worse than T9 — Raju's eyes (grey/white stripes in the white and a fragmented pupil), left eyebrow reduced to a thin crescent, horizontal stripe patches on forehead and at the hair/neck edge; Guddu's right eye white carries a grey cross; Woman1's eyes are grey/white blobs (`Woman1_sheet.png`); a mid-dark mass's edge band is now a stroke and its core a smaller fill, which shrinks pupils/brows and loses small dark features (the recall drop). |
| T9b Opus review | **body outlines accepted; face + tiling regressions → T9c** | 2026-10-06 | Viewed `rigs_t9b/Raju_zoom.png`: jaw, ear, arm, hand and finger outlines are continuous solid strokes at 4× — the prime concern is met on the body. Coverage 95.7 / 96.4 / 95.6 %. Regressions: eyes striped with fragmented pupils, brows thinned, forehead/neck/bag stripe patches, `quality_redesign` red (Hips 64 %), small features 69–74 %. Decisions in T9c. Still no Cast/episode rebuild. |
| T9c protected face boxes, ink-coloured test, streak-free under-paint, every part gets its piece | **built as specified; PARTIAL — 2 gates red (`quality_redesign` Hips 67.5 %, `trace_fidelity::ink_coverage_on_a_synthetic…` discrimination half 27.5 % vs < 20 %) and the eyes/stripes are NOT fixed: Opus decision needed (do not rebuild Cast/episode)** | 2026-10-06 | **Code.** `buzz-scene`: `InkDetect::LocalContrast.max_delta_e_to_line: f64` (INFINITY = off; also off when `line_colour` is None), `ink_mask_local(.., line: Option<(Lab, maxΔE)>)` rejects an ink candidate whose own CIELAB colour is further than that from `options.line_colour`; unit test `a_dark_red_line_is_not_ink_with_a_brown_line_colour_but_a_brown_one_is` (buzz-scene `--lib trace` 45/0). `character.rs`: `character_ink_options(w,h,line_colour)`/`character_thin_ink(w,h,rgba,line_colour)` (signature changed; ΔE limit `CHARACTER_INK_MAX_DELTA_E` = 30; pipeline passes the darkest palette colour), `character_line_colour(completed)`, `protected_face_boxes(skel)` (eyes 15 %, brows 25 %, mouth 15 % via `face::mouth_landmark_box` now `pub(crate)`, min 8 px, eye/brow landmarks conf > 0.1 and >= 2 points), `box_mask`; the ink mask is found on the WHOLE picture, cleared inside the boxes, the ink brush shapes are cut with `boolean Difference` against the boxes; `paint_under_ink` = multi-source 8-connected BFS from every present non-ink pixel outside the boxes (colour passed on unchanged = the seed's), boxes neither donate nor receive; tiling floor `max(CLIP_MIN_PX 400, CLIP_MIN_SHARE 0.01 x owned)` (ascending, largest takes the remainder, as T9b); `BUZZ_TRACE_UNDER_PNG=<path>` makes the pipeline write the under-painted picture; fill trace left at `smooth 0.75 / detail 1.8` (not reverted: the stripes are in the under-painted picture itself). `rig_vs_source` writes `<name>_under.png` (sets that env var) and takes the line colour for its ink metric from the darkest of a 24-colour palette of the source. **Tests (trace_fidelity 17/1).** New: `a_dark_red_plaid_is_a_fill_and_never_ink`, `a_drawn_eye_in_the_protected_box_survives_as_fills_and_has_no_ink`, `a_region_straddling_three_parts_80_15_5_gives_each_part_its_piece`; changed: GRID back to dark red (150,30,30), `inked_fixture` adds an ink strip to the first part's picture outside the matte (the palette's darkest colour must be the ink, else the ΔE gate removes all ink), call sites use `ink_of`/`line_colour_of`. face_rig 5/0, assemble_character 1/0, rig_quality 5/0, trace_character 3/0. **Red gates and why.** (1) `quality_redesign::traced_shapes_stay_localised…` still fails: the offender is a 633 px region, owners Hips 436 / part 9 197; no second owner can reach the 400 px floor, so the region goes whole to Hips (67.5 % own). With the 1 % share rule and NO px floor it passes (tried, reverted) — the 400 px floor and that test conflict for regions under 800 px. (2) The ΔE gate is applied per pixel: anti-aliased edge pixels of a dark line drift away from the line colour, drop out of the mask (thin-ink px 40574 -> 29788 on Raju) and stay as dark seeds, so on the 3 px synthetic outline the fills alone still cover 27.5 % of the ink (passes with a limit of 45+). **Eyes / stripes (by eye, `rigs_t9c/*_zoom.png`, `Raju_under.png`).** (3) The landmark boxes do not sit on the drawn features: on Raju each eye box is a flat rectangle over the UPPER half of the eye and each brow box a strip across the lower part of the brow, so the lower pupil and the top of each brow are outside the box, are ink, and are under-painted away (grey chevron patches in the lower pupils, hatched brows) — the same chevrons show on Guddu and Woman1 pupils. (4) The stripes/hatch on forehead, hairline and jaw are in `_under.png` itself, so they are not the fill trace: the BFS seeds are the dark anti-aliased halo pixels hugging the ink mask, and where the halo is patchy the nearest seed alternates dark / skin (diagonal hatch); dilating the under-paint mask by 2 px removed the hatch (tried, reverted) but cut small features 73.6 -> 62.9 % and coverage 97.0 -> 92.5 %, and also wiped the unprotected pupils. **`rig_vs_source`** (scratch `rigs_t9c/`): Raju `{"mean_de":1.6,"small_features_pct":73.6,"ink_coverage_pct":97.04,"shapes":2574}` (T9b 1.85 / 69.1 / 95.7); Guddu `{"mean_de":2.6,"small_features_pct":75.31,"ink_coverage_pct":93.16,"shapes":4154}` (2.53 / 73.6 / 96.4); Woman1 `{"mean_de":7.35,"small_features_pct":79.69,"ink_coverage_pct":96.73,"shapes":3875}` (7.39 / 69.0 / 95.6). Body outlines are still continuous strokes; dark red bangles/bindi stay red fills; brows are whole but their top edge is patchy; pupils are NOT whole. Still no Cast/episode rebuild. |
| T9c Opus review | **partial → T9d** | 2026-10-06 | Implementer's findings accepted: landmark boxes cover only half the drawn eyes (pupils fragmented), under-paint hatch comes from halo seeds, per-pixel ΔE strips line edges, 400 px² floor conflicts with `quality_redesign`. Body outlines continuous; bangles/bindi red. Numbers: Raju ΔE 1.60 / features 73.6 / coverage 97.0; Guddu 2.60 / 75.3 / 93.2; Woman1 7.35 / 79.7 / 96.7. |
| T9d protect by part masks, eroded seeds, per-line ink colour, 1 % owner rule | **built as specified; tests all green; PARTIAL by eye: the Eyebrow/Mouth PART MASKS are not the drawn brows/mouth, so the brows are NOT protected and come out thin/torn (Opus decision needed; no Cast/episode rebuild)** | 2026-10-06 | **Code.** `trace_lines.rs`: `ink_mask_local` runs the local-contrast + max_luma test first, then `keep_ink_coloured_lines` labels 8-connected components of the candidate mask and keeps a component iff its MEDIAN CIELAB ΔE to `line_colour` <= `max_delta_e_to_line` (< 4 px: the mean); per-pixel path gone, `line_colour: None` / INFINITY = test off. `trace.rs` unit test `a_dark_red_line_is_not_ink_with_a_brown_line_colour_but_a_brown_one_is` now has half-blended anti-aliased edge rows: red rejected whole, brown kept whole (== the test-off mask). `character.rs`: `protected_by_parts` = owner-map pixels of parts named *eye*/*eyebrow*/*mouth* (case-insensitive) dilated 4 px (`dilate_square`, 8-connected rounds); landmark boxes (`protected_face_boxes`) only if no such part owns a pixel; the ink brush shapes are cut with `boolean Difference` against `mask_cut_paths` (the traced outline of the protected mask; boxes in the fallback). `paint_under_ink`: P = ink dilated 1 px minus protected; seeds = present px outside P dilated 1 px (>= 3 px from ink) and unprotected; BFS in distance layers through the px within 2 px of ink (the 1 px ring carries the seed colour but keeps its own), P px repainted with the seed's colour, unreachable P px keep theirs. Owner rule: `CLIP_MIN_PX` removed, qualifying = owners with >= 1 % of owned px (`CLIP_MIN_SHARE`), ascending, largest takes the remainder, `WHOLE_REGION_SMALL_PX` 400 rule kept. `BUZZ_TRACE_PROTECTED_PNG=<path>` writes the protected mask (magenta) for review. **Tests.** trace_fidelity 19/0 (18 + new `a_drawn_eye_owned_by_an_eye_part_survives_whole_with_no_ink`; `eye_fixture(landmarks, as_part)`; the old eye test now covers the landmark-box fallback; the synthetic coverage test's second half passes), quality_redesign 10/0 (Hips 67.5 % gone), face_rig 5/0, assemble_character 1/0 (1 ignored), rig_quality 5/0, trace_character 3/0, buzz-scene `--lib trace` 45/0, buzz-ai lib `under_paint_tests` 4/0 (dilate_square, feature-part names, under-paint takes clean skin and never the halo, no-seed keeps colour). **rig_vs_source** (scratch `rigs_t9d/`): Raju `{"mean_de":1.77,"small_features_pct":72.47,"ink_coverage_pct":95.76,"shapes":1345}` (T9c 1.60 / 73.6 / 97.0 / 2574); Guddu `{"mean_de":2.4,"small_features_pct":72.38,"ink_coverage_pct":97.61,"shapes":3382}` (2.60 / 75.3 / 93.2 / 4154); Woman1 `{"mean_de":7.35,"small_features_pct":75.48,"ink_coverage_pct":94.28,"shapes":2796}` (7.35 / 79.7 / 96.7 / 3875). Coverage >= 90 on all three; Raju ΔE 1.77 <= 2.0; small features fell (72.5 / 72.4 / 75.5, accept. wanted >= 85 / 80). **By eye.** Pupils are now whole dark fills on Raju and Guddu (clean, highlight inside), Woman1 pupils are whole but with highlight holes; the BROWS are not whole: `Raju_protected.png` (magenta = protected) shows the Eyebrow parts own blobs of hair, temple and jaw, the Mouth part a jaw patch, the Eye parts only the pupil area, so the drawn brows are outside the region, are thin ink (a 12 px brow is under 2 x max_line_radius 9.65), are under-painted to skin (Raju `_under.png` has NO brows, faint diamond ghosts on the forehead where they were) and render as a thin strip or crescent (Guddu's torn into patches). The hatch on skin is gone in `_under.png` except those faint brow ghosts; arm, hand and jaw outlines stay continuous (Raju forearm/hand, Guddu arm, Woman1 arm), but Guddu/Woman1 hands show a scribbled ink loop between thumb and fingers and the bangles/bag stripes are messy. Open: protect brows by landmark box (T9c) UNION the part mask, or raise the ink gate for thick dark masses (`max_line_radius`) so a brow stays a fill; mouth is replaced by the mouth chart anyway. |
| T9d Opus review | **partial → T9e** | 2026-10-06 | Accepted: pupils whole, skin hatch gone, `quality_redesign` 10/10, trace_fidelity 19/19, coverage 95.8 / 97.6 / 94.3 %, Raju ΔE 1.77. Open: brows thin/torn (classed as thin ink at radius 6 < 9.65; the segmenter's brow/mouth parts are the wrong pixels), hand loop scribbles on Guddu/Woman1, small features 72–75 %. Decision: masses by compactness (T9e). |
| T9e masses by compactness; protected masses in the head; hand loops | **built; tests green; by eye: brows, pupils whole, hand loop gone, outlines continuous; small features NOT >= 80 % (68-73 %)** | 2026-10-06 | **Code.** `trace_lines.rs`: `classify_ink` (ink mask, `clean`, `fill_thin_slits`, `classify_components`): 8-connected components of the candidate mask; per component `area`, `skeleton_len` (Zhang-Suen + staircase cleanup run ONCE over the whole mask, pixels counted by component label), pen width = 2 x median EDT radius; `line_width_typical` = `median_of` all components, clamped [1.5, 6]; MASS iff `area / skeleton_len > 2.5 x line_width_typical` (skeleton < 3 px: `area / max(bbox_w, bbox_h)`); `line` = the line components minus `strip_masses(max_line_radius)` cores (the radius is only the upper bound now), `mass` = the rest of the ink. `traced_ink` with `ink_masses: false` returns `line` (the `ink_masses: true` path is untouched). `trace.rs`: `pub struct InkComponents { line_mask, mass_mask, line_width_typical }`, `pub fn ink_components(w, h, rgba, opts)`. `character.rs`: ink pass and under-paint use `line_mask` (P = line grown 1 px; a mass is never 8-adjacent to a line of another component, so masses are never repainted); protected = Eye-part pixels grown 4 px (landmark boxes only if no Eye part) UNION (`mass_mask` AND head-part owner pixels, grown 2 px; `protected_head_masses`); the Eyebrow/Mouth parts are no longer protected (`is_eye_part` = contains "eye", not "brow"; `is_head_part` = contains "head"); ink brush shapes cut by the traced outline of each protected region. `rig_vs_source` also writes `<name>_ink_hand.png` and `<name>_ink_classes_hand.png` (4x crops of the arm box: ink layer; source with LINE red / MASS blue). **Hand loop cause: (a) skeleton artefact, not (b) a matte hole.** In `Guddu_ink_classes_hand.png` the folded fingers are three 3 px lines a pen-width apart: the ink mask is one thick band pierced by two enclosed paper slits ~3 px x 25 px; the skeleton loops round each slit (cycles of 50+ px through junctions) and the brush turns them into crossing ribbons. Tried the plan's `drop closed loops < 3 x min_line_length`: removed 33 tiny pure cycles but left the scribble untouched (the loops are multi-edge cycles of 50+ px, not pure cycles), so it was REVERTED (small features fell 70.7 -> 69.9 for nothing). The fix that works is at the mask: `fill_thin_slits` turns every enclosed paper component whose widest point is <= 0.35 x `max_line_radius` (about 3 px on these pictures) from ink to ink before classification, so a fused band is one solid line (a ring's hole is far wider and stays). Deviation from the plan's list, flagged for review: it is a mask-level fix, applies only with `ink_masses: false`, one function + one constant, easy to drop. **Tests.** trace_fidelity 21/0 (19 + `a_brow_bar_in_the_head_is_a_fill_and_an_eye_ring_is_ink`, `the_under_paint_leaves_a_brow_mass_untouched`; the synthetic brow is a 12 px bar under `max_line_radius` 6.93, so only compactness makes it a mass), quality_redesign 10/0, face_rig 5/0, assemble_character 1/0 (1 ignored), rig_quality 5/0, trace_character 3/0, buzz-scene `--lib trace` 50/0 (+5: `a_12_by_60_dark_bar_is_a_mass`, `a_pressed_stroke_stays_one_line`, `a_ten_pixel_radius_disc_is_a_mass`, `a_three_pixel_ring_is_a_line`, `thin_slits_between_fused_lines_are_filled_but_a_rings_hole_is_not`), buzz-ai lib `under_paint_tests` 5/0 (+`under_paint_from_the_line_mask_leaves_a_brow_mass_untouched`; the old part-name test became `eye_and_head_parts_are_found_by_name_ignoring_case`). **rig_vs_source** (scratch `rigs_t9e/`): Raju `{"mean_de":1.81,"small_features_pct":72.47,"ink_coverage_pct":96.02,"shapes":1053,"ink_shapes":190}` (T9d 1.77 / 72.5 / 95.8 / 1345); Guddu `{"mean_de":2.55,"small_features_pct":68.2,"ink_coverage_pct":96.17,"shapes":3206,"ink_shapes":209}` (2.40 / 72.4 / 97.6 / 3382); Woman1 `{"mean_de":7.36,"small_features_pct":73.18,"ink_coverage_pct":94.36,"shapes":2834,"ink_shapes":121}` (7.35 / 75.5 / 94.3 / 2796). Coverage >= 90 on all three, Raju ΔE 1.81 <= 2.0; small features are BELOW the 80 % acceptance on all three and fell on Guddu / Woman1 (the metric counts 12-400 px single-colour components; the filled slit pockets and thin dark fragments are among them). **By eye** (`_zoom.png`, `_under.png`): brows are whole dark fills on Raju (both) and Guddu (right; the left joins the hair mass) and mostly whole on Woman1; pupils are whole with their highlight; Guddu's hand loop is gone (one solid dark band under the fingers, in the source a dense dark fold); body outlines (jaw, arm, hand, fingers) are continuous strokes and `Raju_under.png` shows the outlines painted out with brows, pupils, hair, shoes and belt buckle left as drawn and no hatch. Still no Cast/episode rebuild. |
| T9e Opus review | **accepted — outlines meet the prime concern; colour-detail metric left open** | 2026-10-06 | Viewed `rigs_t9e/Raju_zoom.png`, `Guddu_zoom.png`: brows whole, eyes and pupils whole (with highlights), jaw/ear/neck/arm/hand/finger outlines continuous smooth strokes, hair/shoes/buckle solid fills, hand loop gone, no skin hatch. Tests: trace_fidelity 21/21, quality_redesign 10/10, buzz-scene trace 50/50, under_paint 5/5. Numbers: coverage 96.0 / 96.2 / 94.4 %; Raju ΔE 1.81; small features 72.5 / 68.2 / 73.2 % (below the 80 % target — a colour/detail item for later, per the user's "colour can be added later"). Accepted the `fill_thin_slits` deviation (hand slits). Open for later: a brow fused to the hair joins the hair mass (Guddu left), bag stripes messy, Raju's chart Rest mouth is a line not the smile (chart design). **Rebuilding the 12 rigs and the episode with this pipeline.** |
| T6 (second pass, T9e pipeline) | **rigs 12/12 rebuilt (4 m 21 s, Cast replaced); episode_v2 rebuilt (3 m 14 s, 21 stills)** | 2026-10-06 | Sheets `rigs_t9e_a.png`/`_b.png`: every rig has continuous smooth outlines and solid faces; patterns kept. Still `episode_v2/stills/scene05_raju_arrives_f0542.png`: four rigs crisp, no rim. **Open polish (T11):** a small black rectangle floating right of Lalten's torso (front and back views, same spot); two stray marks beside Woman1 3-4's head; a brow fused to the hair joins the hair mass (Guddu left). |
| T10 wrists, ankles, brows as their own bones (`biped_full`, 17 slots) | **built as specified; tests green; Raju metrics unchanged; Opus review pending** | 2026-10-06 | **Code.** `buzz-rig/pattern.rs`: `RigPattern::biped_full()` ("Biped Full") = `biped()`'s 11 slots + `11 Wrist L` (parent 4) / `12 Wrist R` (6) / `13 Ankle L` (8, rest angle −π/2+0.2) / `14 Ankle R` (10) / `15 Brow L` (2) / `16 Brow R` (2), all optional; inside `biped_full` only, "hand" is taken off Elbow and "foot"/"boot" off Knee (they are the new slots' aliases; `biped()` unchanged); registered LAST in `builtin()` so `RigPattern::named("Biped Full")` and the Rigging panel's pattern list find it (no `rig_panel.rs` change). `buzz-ai/character.rs`: `assemble_character` uses `biped_full()`; `map_part_to_biped_slot` hand/wrist/palm/finger → 11/12, foot/ankle/toe → 13/14, brow (by left/right in the name) → 15/16, all before the elbow/knee/head rules (no side → old rules); `biped_joints_from_skeleton` now 17 entries: Elbow = elbow→wrist, Knee = knee→ankle, 11/12 = Wrist→Hand joint, 13/14 = Ankle→Foot joint, 15/16 = brow inner end→outer end by x (inner = nearest the nose, else the middle of both brows; needs 2 confident landmarks), and `assemble_character` falls back to the brow part's drawn bounds across its vertical centre when a brow has no landmarks (none → pattern default); parts of slots ≥ 11 are always bound AFTER the primaries (as hands/feet/brows were as subparts), so paint order is unchanged. `skeleton()` bones "Hand L/R"/"Foot L/R" → "Wrist L/R"/"Ankle L/R" (joints keep their names); `bone_depth`, the cap width factor and the legs-fallback bone lookup know both names. `part_masks` step 5b: after the owner map, a pixel owned by the forearm or the hand (shin or foot) is the hand (foot) iff it lies at or past the line through the wrist (ankle) perpendicular to the forearm (shin) bone, else the forearm (shin). **Tests.** buzz-rig 96/0 (2 new: first 11 slots equal `biped()`'s in name/parent/angle/length/side/required, 17 total, parents as above; hand/foot/brow names land on their own slots); buzz-ai new `tests/biped_full.rs` 8/0 + 1 ignored (17 bones, first 11 = `biped()`'s names/parents; bones at the wrist/ankle/brow ends; each part on its slot's bone; Wrist L +30° moves only the hand part's posed bounds (elbow turn carries the hand); Brow L +30° moves only the brow part; 6 standard poses (11 angles) apply to the 17-bone rig and leave bones 11–16 at rest; save/load through `buzz_doc::format` keeps 17 bones, pattern name, bindings and 17-angle poses; wrist/ankle-line owner test over a whole synthetic picture; ignored `raju_rig_has_seventeen_bones_with_models` prints the bones). Existing green: assemble_character 1/0 (+1 ignored), face_rig 5, trace_fidelity 21, quality_redesign 10, rig_quality 5, motion_and_poses 6, rerig_vector_character 5, part_masks 3, trace_character 3, complete_parts 2, lib 75; buzz-act 124 + 13 + 4 + 5 + 14 + 6 + 4 (untouched); buzz-doc 230/0 (no change needed); buzz-ui `--lib rig` 15, `--lib ai_character` 4. **Changed tests:** `assemble_character` (11 → 17 bones, first 11 equal `biped()`'s; also in the ignored model test), `quality_redesign::foot_dom_color_is_not_the_pale_grey_fallback_on_oldman` (parts "Foot L/R" → "Ankle L/R", the renamed bones). **Raju** (`rig_vs_source`, scratch `rigs_t10/`): `{"edge_mean_px":0.27,"edge_p95_px":1.0,"ink_coverage_pct":95.87,"mean_de":1.81,"shapes":1055,"small_features_pct":72.47}` (T9e: 1.81 / 96.02 / 1053 / 72.47). Bones: Hips, Chest, Head, Shoulder L, Elbow L, Shoulder R, Elbow R, Thigh L, Knee L, Thigh R, Knee R, Wrist L, Wrist R, Ankle L, Ankle R, Brow L, Brow R. **Reviewer risks:** (1) the Eyebrow PART is the segmenter's patch, not the drawn brow: on Raju "Left Eyebrow" draws a 100 × 118 px blob (x 424–525, y 79–197; landmarks sit at y 132–151), so Brow L turns that patch, while the drawn brow fills sit in whichever part owns their pixels (T9e); (2) the wizard (`apply_edited_joints` → `rebuild_bones` in `buzz-ui/ai_character_dialog.rs`, outside this brief) still rebuilds bones as "Hand L/R"/"Foot L/R", so the production parts keep those names — every rule matches both; (3) `series_setup`/`episode_build` not run (only `cargo check`). |
| T10 Opus review | **accepted (tests re-run by Opus: buzz-rig 96/96, biped_full 8/8 +1 ignored, assemble_character 1/1, motion_and_poses 6/6, quality_redesign 10/10, trace_fidelity 21/21); one carried defect → T11** | 2026-10-06 | 17 bones on Raju (`Hips … Knee R, Wrist L, Wrist R, Ankle L, Ankle R, Brow L, Brow R`); buzz-rig 96/96, buzz-ai suites green incl. new `biped_full` 8 (+1 model-gated); buzz-doc round-trip proven; buzz-act untouched; Raju metrics unchanged. Carried: the segmenter's brow part is a forehead patch, so Brow L/R would turn skin — T11 derives the brow part from the drawn dark mass. The Cast is NOT rebuilt with 17 bones until T11 lands (one rebuild, not two). |
| T11 brow part = drawn brow; stray fragments (Lalten rectangle, Woman1 3-4 marks) | **partial (stopped on request): brow part + stray-distance rule built and tested; Lalten rectangle NOT fixed (cause narrowed, not a traced shape); Woman1 marks not reproduced; Opus review pending** | 2026-10-06 | **Code** (`character.rs`, only): `trace_character_whole_with` now runs `ink_components` up front, then `brow_parts_from_masses`: per side, brow part = `mass_mask` components (8-connected) whose centroid is in `brow_search_box` (brow landmark box padded 60 %, min 12 px, kept above the same-side eye landmark box top but never cut into the brow landmarks), grown 1 px, only over head-owned/unowned pixels; the segmenter's brow patch goes back to the Head; `completed` is shadowed by a corrected copy (brow `rgba` = source under its new pixels, head `rgba` filled where it took the patch back) so hidden-fill / dark-fill logic agree; the palette/line colour still come from the ORIGINAL parts; `protected_head_masses` now counts brow-owned masses too; `part_masks` untouched. Stray rule: unowned region/ink shape (`nearest_bone_part` now returns the distance) is dropped when centroid > 1.5 x `limb_radius_at_head` from the bone AND not attached (`stray_anchor_mask`: owned pixels + any 8-connected unowned blob within 2 px of an owned pixel, grown 1 px); counted, `BUZZ_TRACE_INK_LOG=1` prints per-shape decisions (shapes >= 50 px) and the drop count and each brow part's pixels. `BUZZ_TRACE_PARTS_PNG=<path>` writes every part's caps/shapes/ink in its own colour (dropped shapes red); `rig_vs_source` sets it to `<name>_parts.png`. **Tests.** trace_fidelity 23/0 (+2: `an_unowned_blob_far_from_every_bone_and_touching_nothing_is_dropped`, `the_brow_part_is_the_drawn_brow_and_the_head_keeps_the_skin` — synthetic 50 x 12 bar (not 12 x 60), oversized 80 x 50 patch part, union box within 2 px, head keeps skin); CHANGED `a_region_no_part_owns_goes_whole_to_the_nearest_bone_part` (bags moved to TOUCH the body, area tolerance 0.5 % -> 15 % because the bag now sits on the 10 px outline); biped_full 8/0 (+1 ignored), face_rig 5, assemble_character 1 (+1 ignored), quality_redesign 10, rig_quality 5, part_masks 3, motion_and_poses 6 all green; `cargo check --release -p buzz-ai -p buzz-scene` ok; buzz-scene `--lib trace` NOT re-run (no buzz-scene change). **Measured** (`rig_vs_source`, scratch `rigs_t11/`): Raju `{"edge_mean_px":0.27,"edge_p95_px":1.0,"ink_coverage_pct":97.06,"mean_de":1.76,"shapes":1045,"small_features_pct":71.91}` vs T10 1.81 / 95.87 / 1055 / 72.47 (dE within 0.1; coverage +1.19, just over the +-1 asked; both brows own 778 / 689 px). Lalten: Right Eyebrow owns 719 px (a proper arched brow, by eye), Left Eyebrow 0 px (brow fused to hair, stays in the head). **Lalten rectangle:** an axis-aligned 64 x 11 px block of colour (41,22,14) at source (656..719, 370..380), on the vest below the chin; it is in NO traced shape, hidden fill or cap (`Lalten_parts.png` shows plain Hips there) and no unowned shape exists (0 dropped, none logged), so it is NOT the stray/`fill_thin_slits`/matte-hole path; it comes after the trace, most likely the face rig's mouth symbol (`restyle_mouth_visemes_at` / `build_face_rig` in face.rs) placed at mouth landmarks that sit on the chest — UNVERIFIED, not fixed. **Woman1 3-4:** no stray marks visible in the rig_vs_source sheet and 0 unowned shapes; both brow parts own 0 px (no mass qualified; brows are lines/fused), so Brow L/R have no artwork there. **Not done:** series_setup into scratch (stopped), Lalten fix, Opus by-eye review of brows on all rigs. |
| T11 Opus review (wrap-up) | **partial, accepted as left** | 2026-10-06 | Brow parts now come from the drawn dark mass (Raju 778/689 px, Lalten right brow 719 px; a brow fused to the hair stays in the head); stray unowned fragments > 1.5 × limb radius from their bone and touching nothing are dropped (0 dropped on the three tested). trace_fidelity 23/23, biped_full 8/8, other buzz-ai suites green. **Lalten's floating rectangle is NOT a tracer artefact**: an axis-aligned 64×11 px block on the vest, present in no traced shape — most likely the placeholder mouth symbol placed at mouth landmarks that sit on the chest (`face::restyle_mouth_visemes_at` / `build_face_rig`). Woman1 3-4's marks did not reproduce in `rig_vs_source`. **Open for the next session:** Lalten mouth placement; Woman1 3-4 marks in the Cast build; rebuild the Cast + episode with the 17-bone pipeline (not done — the Cast on disk is the T9e 11-bone build). |

## Diagnosis (measured 2026-10-06)

The **tracer itself is not the problem.** `trace_png` on the Raju source:

| Mode | Shapes | Time | mean ΔE | edge p95 | small features | By eye |
|---|---|---|---|---|---|---|
| `colour:24` (`TraceMode::Outline`, regions only) | 1403 fills | 893 ms | 0.64 | 1.0 px | 88.8 % | matches the source: plaid, bag, pens, watch, face, line weights |
| `colourlines:24` (what the pipeline uses) | 1403 fills + 3688 strokes | 999 ms | 0.84 | 328 px | 57.5 % | dotted plaid, double edges, specks ringed with ink |
| `auto` → LineArt | 69 strokes + 16 fills | 457 ms | 5.61 | 2.0 px | 14.4 % | clean ink, no colour |

The rig preview, next to the source (scratch `compare_head_torso.png`),
loses the plaid, the bag, the eyebrows and the pens; gains two-tone beige
clouds on the shirt, black blotches, dashed outlines, stair-stepped edges at
every part cut, a floating stroke fragment at the shoulder, and generic
ellipse eyes.

### Where each defect comes from (`crates/buzz-ai/src/character.rs`)

1. **`trace_character_whole` (~2462) traces a composite of the *completed*
   parts after `clean_composite_regions` (~2196).** The cleaner merges every
   patch under a size threshold into its surroundings: that is what deletes
   the plaid and turns the shirt into two-tone clouds. The composite is also
   posterised to the k-means palette there.
2. **`TraceMode::ColourLines`** adds a uniform-width stroke along every
   colour boundary and the silhouette (`build_outline_strokes`, trace.rs
   ~1002). On inked art the ink is already a region; the strokes double it
   and ring every speck.
3. **Every fill is snapped to a 24-colour palette** (`nearest_palette_color`,
   ~2590) even though the tracer already returned the exact mean colour of
   the region (`region_fill_colour`, trace.rs ~597).
4. **Every fill is boolean-clipped** against `part_boundary` (~2532), a
   trace of the pixel owner mask at `smooth: 0.6`, so a region sitting 99 %
   inside one part still gets cut along a stair-stepped pixel edge.
5. **Strokes are cut per pixel run of the owner map** (`cut_stroke_by_owner`,
   ~1865): the dashes.
6. **`assemble_character` (~3037) deletes the drawn eyes** (`eye_removal_boxes`)
   and `face::build_face_rig` (face.rs ~520) replaces them with ellipse
   whites, round pupils and a synthetic lid.

Completion (`complete_parts`, ~1263) is a flat dominant-colour fill of the
hidden capsule / joint-cap area (LaMa only in `complete_parts_high_quality`).
It is right for what is hidden and wrong as a tracing input for what is
visible.

## What "right" is

At rest, the rig rendered at the source's size **is the source drawing**:
the ink as the artist drew it (traced as filled regions with their own
weights), every fill its exact colour, patterns kept, small features kept.
Parts are cut only where a region genuinely straddles two parts, along a
smooth cut; a part's hidden completion sits *behind* its visible shapes and
only shows when a joint opens. The face keeps its drawn eyes and mouth; the
blink lid and the movable pupil are derived from the drawn eye.

## T1 — visible-first trace (one implementer run)

In `trace_character_whole`:

1. Build `visible_rgba`: the source pixel wherever `matte > 128`, transparent
   elsewhere. **Do not** run `clean_composite_regions` on it and do not
   posterise it. (Keep the function for the mouth chart, which still uses it.)
2. Trace `visible_rgba` once with `TraceMode::Outline`, `colours: 32`,
   default `speckle`, default `smooth`/`detail`. The palette here only drives
   the region flood; fill colours are the tracer's exact per-region means.
3. **No palette snapping of fills.** Delete the `nearest_palette_color` step
   on `report.shapes`. The quantised palette stays for the line colour, caps,
   lids and mouth only.
4. **Boundary strokes become opt-in.** New `TraceCharacterOptions {
   uniform_width: f64, outline_strokes: bool }` (default `false`), threaded
   from `AiCharacterState` (dialog: a checkbox "Outline strokes" next to the
   width control; `series_setup` and `character_from_image` use the default).
   When on, trace in `ColourLines` as today. When off, `uniform_width` still
   sizes the caps' arcs, the lid outline and the mouth lines.
5. Keep the dark-fill guard (`DARK_FILL_AREA_GUARD_PX`, `region_is_dark`).

Tests (`crates/buzz-ai/tests/trace_character.rs` or a new
`trace_fidelity.rs`, synthetic images, no models):

- a shirt with a 3-px plaid grid on a two-part body keeps ≥ 90 % of the grid
  lines as regions (count regions whose colour is the grid colour);
- every fill's colour is within ΔE 2 of the source pixels under it;
- with `outline_strokes: false` the result has zero stroked shapes; with
  `true` it has some.

## T2 — part assignment without stair-steps (same run as T1 if it fits, else next)

1. Per region: `area_by_owner` as today. Let `top` be the largest share.
   - `top ≥ 0.92`, or region area < 400 px² at source resolution → assign the
     **whole** region to that owner, unclipped.
   - else clip against every owner with share ≥ 0.08 (never the `touched`
     fallback: a region whose owners all have < 8 % goes whole to `top`).
2. **Smooth part boundaries.** Trace each owner mask with `TraceOptions {
   mode: Outline, smooth: 1.0, speckle: 64, .. }` then simplify with a
   tolerance of `max(1.5 px, limb_width / 12)` where `limb_width` is the
   part's bone radius from the distance transform (reuse
   `limb_radius_at_head`); union the pieces as today. Report the vertex count
   per boundary before/after in the trace report.
3. Clipping stays `buzz_geom::boolean(…, Intersect)`. **Invariant:** the
   clipped pieces of one region union back to the original within 1 % of its
   area (test with a synthetic two-part body and a region straddling the
   joint).
4. Strokes (only when `outline_strokes` is on) are assigned whole to the
   owner holding the majority of their length, never cut — a stroke that
   crosses a joint is cut once at the joint cut line (the smoothed boundary),
   not per pixel run. Remove `cut_stroke_by_owner` or keep it only for this.

## T3 — hidden completion per part, behind the visible shapes

For every bone part: `hidden = completed.alpha > 128 ∧ owner != self`. Trace
`hidden` (as a 2-colour mask, `TraceMode::Outline`, `speckle: 16`) and push
the result, filled with `part_dom_color`, at the **front of the part's
shape list** (behind everything visible), replacing today's `part_boundary`
base-fill insert (~2697, ~2769). Joint caps unchanged (`caps`, bound at the
back by `assemble_character`). Test: a part fully visible gets no hidden
fill; a part half covered by another gets one, and it lies entirely under
the covering part's owner pixels.

## T4 — the face keeps its drawn eyes (own run)

1. `assemble_character`: stop deleting the eye artwork (`eye_removal_boxes`
   filter). The drawn eye stays in the head part exactly as traced.
2. `face::build_face_rig`:
   - **Pupil**: the darkest traced fill(s) inside the eye's landmark box
     (lightness L* < 35, area ≥ 4 px²), moved out of the head part into the
     pupil symbol at their own centre (so look-at can move them). If none
     is found, the synthetic pupil as today.
   - **Eye white**: no synthetic white. The composite eye symbol holds the
     pupil and the lid only; the white is the drawn one in the head part.
   - **Lid**: the 3-frame lid as today, but sized to the bounding box of the
     traced light fills inside the eye box (not the padded landmark box),
     filled with the skin colour sampled from the head part's largest fill
     touching that box, outlined in the line colour at `uniform_width`.
   - `set_eye_look_at`, `set_eye_blink`, the Cast panel and `eye_variants`
     in `episode.rs` keep working (the composite symbol's layer names
     "Pupil"/"Eyelid" stay).
3. Mouth unchanged (`split_mouth_shapes`, `restyle_mouth_visemes_at`,
   mouth charts). Verify the traced lips still land in `mouth_underlay`.

Tests (`tests/face_rig.rs`): on a synthetic face with drawn eyes the head part
still contains the eye-white fills; the pupil symbol holds a dark fill whose
centre is within 2 px of the drawn pupil's; frame 2 of the lid covers the
eye-white bounds.

## T7 — specks merge into their neighbour (buzz-scene `trace_regions`)

`trace.rs` ~477: a flooded region with fewer cells than `speckle` is counted
and **dropped**, which leaves a hole where the background shows through.
Before T3 the whole-part base fill hid those holes; now they are the white
specks on Woman1's sari and Guddu's shirt. Fix in the tracer (so Trace
Bitmap gains it too):

1. Keep a `region_of: Vec<u32>` cell → kept-region index while flooding.
   Collect specks instead of dropping them.
2. After the flood, for each speck find the kept region most of its cells'
   4-neighbours belong to and append the speck's cells to that region
   (repeat until no speck changes, so a speck touching only other specks
   follows them). A speck with no kept neighbour at all (isolated in
   transparent / background cells) is dropped as today.
3. `report.specks` keeps counting them; the message says "merged N specks
   into their neighbours" (and "dropped M" for the isolated ones).
4. `build_region_shape` then sees the merged cells, so the neighbour's shape
   covers the hole; `region_fill_colour` is still the interior mean.

Tests (buzz-scene): a flat square with three 2×2 off-colour specks inside
traces to exactly one region whose rasterised coverage has no hole; an
isolated speck in transparent surroundings is still dropped; a speck
touching two regions joins the one it shares the longer border with; the
existing trace tests stay green (adjust only the ones that assert the old
"dropped" wording/counts).

Acceptance: `rig_vs_source` Woman1 mean ΔE ≤ 3.5 and small features ≥ 85 %
(the sari's print is the hardest case in the cast), Guddu/Raju not worse;
no white specks by eye on the sheets.

## T9 — the ink as brush strokes: smooth, solid outlines (user's prime concern)

User, 2026-10-06: "make sure the outlines of body, limbs, face are solid
lines, smooth, as if drawn by the brush tool, no irregularity — that is the
prime concern; colour can be added later."

Today the ink is traced as **filled regions** (T1): exact to the source but
pixel-stepped and wobbly at 2–4× (`eyes_zoom.png`, `guddu_head_zoom.png`).
`buzz_scene::trace_lines` already does what the user describes — skeleton,
junctions carried through, measured width smoothed along the line, a
Catmull-Rom fit and `LineStyle::Brush` ("the exact effect of the brush
tool"); the `auto` run on Raju's source (`raju_auto.png`) is the look wanted.

1. **Ink layer.** In `trace_character_whole_with`, run `trace_lines`
   (`TraceMode::LineArt`, `LineStyle::Brush`, `line_smooth: 0.7`,
   `fill_enclosed: false`, `max_line_radius` = `long_edge / 150` clamped
   to 4–12 px, `min_line_length: 6`, `ink_threshold: None` (Otsu) — if Otsu
   swallows dark cloth, cap the threshold so only the darkest 12 % of luma
   counts as ink) on the visible picture. Its brush fills (and the "mass"
   regions it traces for ink too fat to be a line) are the outlines.
2. **Fills under the ink.** Before the region trace, paint every ink-mask
   pixel with the colour of its nearest non-ink pixel (iterative 4-neighbour
   dilation of the non-ink labels into the mask), so fills run *under* the
   strokes and no dark "line region" remains to peek out from a slightly
   thinner stroke. Trace fills with `smooth: 0.75`, `detail: 1.8` (the ink
   now carries the detail, so fill edges may be rounder).
3. **Per part**: fills first (T2/T8 rules), ink on top. A stroke goes whole to
   the owner holding most of its length; one that crosses a part boundary
   (a limb outline running shoulder → hand) is **cut once at the crossing**
   (the smoothed boundary) so the limb can bend; both halves keep their
   width profile. `TracedPart` gets an `ink: Vec<ShapeData>` list drawn after
   `shapes`; `assemble_character` appends it on top. The face selection (T4)
   treats `ink` shapes inside the eye box as outline (White layer), never as
   pupil, whatever their lightness.
4. **Measure smoothness**: `rig_vs_source` prints `ink_vertices_per_100px`
   (path vertices of ink shapes per 100 px of their outline length; a smooth
   brush outline is a few control points per 100 px, a staircase is dozens)
   and writes `<name>_ink.png` (ink layer alone, 2×) and a 4× crop of the
   face and one arm for review by eye.

Acceptance (by eye first): at 4× the outlines of face, arms and legs are
continuous, solid, smoothly varying in width, with no staircase and no
breaks except at joints; `ink_vertices_per_100px ≤ 10`; no stroke fragments
shorter than 6 px; Raju/Guddu/Woman1 metrics not worse than T8's by more
than ΔE 0.5 (fills are now partly covered by strokes, so a small shift is
expected); the three sheets look like the sources.

## T9b — ink detection by local contrast; largest owner takes the remainder

T9 review (`rigs_t9/Raju_zoom.png`, `Raju_ink.png`): the global luma cut
(Otsu, capped at the darkest 12 %) keeps hair, belt, shoes and the bag as ink
but misses the dark-brown 2–3 px outlines of face, jaw, ears, arms and hands
(blurred luma 60–100, above the cap of 39), so they come out dashed. The
`auto` run that looked right used Otsu over the whole picture *including the
white surround* (threshold 155), which is why it caught every line.

1. **Ink = thin and locally dark.** `ink_mask_local(w, h, rgba, opts)`: a
   pixel is ink when its 3×3-blurred luma is `< ratio × background` where
   `background` = the 80th-percentile luma of the opaque pixels in a window
   of `2.5 × max_line_radius` around it (box percentile via two passes or a
   sliding histogram), with `ratio = 0.62`, and its luma is `< 150` absolute.
   A dark garment's interior is not ink (its background is itself dark);
   its edge against skin or paper is a thin band → a stroke along the
   silhouette, which is what the source draws there. Expose it as
   `TraceOptions::ink_detect: InkDetect::{Global(Option<u8>), LocalContrast
   { ratio, max_luma }}` (default `Global` for Trace Bitmap, so nothing else
   changes); the character pipeline uses `LocalContrast`.
2. **Strokes only.** Masses (ink fatter than `max_line_radius`) are not
   emitted by the ink layer in the character pipeline (`masses: false`
   option on the line trace); they stay fills with their true colour. The
   under-painting of the fills uses only the thin-ink mask.
3. **Owner order.** In the T8 tiling, process qualifying owners in
   **ascending** share and let the **largest** owner take the remainder, so
   slivers outside every simplified boundary land on the dominant part, not
   the smallest (fixes `quality_redesign::traced_shapes_stay_localised…`).
4. **Metric.** Replace `ink_vertices_per_100px` (meaningless: `fluid_outline`
   is a dense polyline by design) with `ink_coverage_pct`: the share of the
   source's thin-ink pixels that lie within 2 px of rendered ink (rig
   render, dark pixels L* < 45) — continuity of the outlines. Keep the 4×
   face/arm crops.

Acceptance: `ink_coverage_pct ≥ 90` on Raju, Guddu, Woman1; by eye at 4× the
outlines of face, arms, hands and legs are continuous solid strokes; no
white holes in hair/shoe masses (they are fills again); `quality_redesign`
green; Raju ΔE ≤ 2.0, features ≥ 85 %.

## T9c — keep the face features; ink-coloured only; streak-free under-paint; every part gets its piece

T9b review (`rigs_t9b/Raju_zoom.png`): body outlines are continuous solid
strokes now (coverage 95.6–96.4 %). Regressions: eyes striped grey/white with
fragmented pupils, brows thinned to crescents (a mid-dark feature's edge band
became a stroke and its interior a smaller fill); horizontal stripe patches
on the forehead, neck and bag (the under-paint); `quality_redesign` still red
(Hips 64 % own pixels) because owners under 8 % never get a piece and the
remainder owner takes their pixels. Small features fell to 69–74 %.

1. **Protected feature boxes.** The eye boxes (T4's `eye_removal_boxes`
   padded 15 %), the brow boxes (brow landmark bbox padded 25 %) and the mouth
   box (`mouth_landmark_box`) are **excluded from the ink mask and from the
   under-paint**: inside them the picture is traced exactly as T8 did (fills
   only, drawn eyes/brows/mouth). The jaw, ears, nose line and hair edge stay
   ink. (Mouth charts replace the mouth anyway.)
2. **Ink is ink-coloured.** Add to the local-contrast test: CIELAB ΔE between
   the pixel and the line colour (the darkest palette colour) ≤ 30. A
   saturated dark red/green/purple patch (plaid line, sari print, towel) is
   not ink and stays a fill with its own colour; dark-grey/brown/black edges
   still are. Test: a dark red 3-px line on cream is NOT ink; a dark brown
   one is.
3. **Under-paint without streaks.** Replace the current repaint with a
   multi-source BFS from every non-ink opaque pixel (8-connected, processed
   in distance layers) that copies the *source* pixel's colour of the nearest
   non-ink pixel, so bands between two colours get the nearer side, never
   row-wise streaks. Write `<name>_under.png` from `rig_vs_source` for
   review. If the stripes persist, they come from the fill trace (`smooth
   0.75 / detail 1.8` on skin gradients): then go back to `smooth 0.5 /
   detail 1.5` for fills and say so.
4. **Every part gets its piece.** Qualifying owners for a straddling region
   = every owner with ≥ max(400 px², 1 % of the region) in it (not ≥ 8 %);
   tile ascending by share; the largest takes the remainder. `quality_redesign::
   traced_shapes_stay_localised_to_their_own_part_on_pujari` must pass
   unchanged.

Acceptance: `ink_coverage_pct ≥ 90` (unchanged), small features ≥ 85 % on
Raju and ≥ 80 % on Guddu/Woman1, Raju ΔE ≤ 2.0, `quality_redesign` green;
by eye at 4×: drawn eyes with whole pupils and full brows, no stripes on
skin, body outlines continuous as in T9b.

## T9d — protect by part masks; eroded under-paint seeds; ink colour per line; 1 % owner rule

T9c review: body outlines still continuous (coverage 93–97 %), bangles/bindi
stay red. Pupils still fragmented because the **landmark** eye boxes cover
only the upper half of each drawn eye and the brow boxes only a strip; the
hatch on skin is in `_under.png` itself: BFS seeds include the dark
anti-aliased halo next to the ink, so the nearest seed alternates dark/skin;
the per-pixel ΔE gate strips a dark line's anti-aliased edge pixels out of
the mask; the 400 px² owner floor conflicts with `quality_redesign` on
regions under 800 px².

1. **Protected region = the segmenter's face feature parts.** From the owner
   map: every pixel owned by a part whose name contains "Eye", "Eyebrow" or
   "Mouth", dilated by 4 px. No ink detection and no under-paint there (T8
   behaviour). Drop the landmark boxes for this purpose.
2. **Under-paint seeds eroded.** Paint set = thin-ink mask dilated by 1 px
   (the halo is repainted too). Seeds = opaque pixels outside the paint set
   dilated by a further 1 px (every seed ≥ 2 px from ink), outside the
   protected region. BFS as T9c. The hatch must be gone in `_under.png`.
3. **Ink colour per line, not per pixel.** Run the local-contrast test
   without the ΔE gate, take 8-connected components of the candidate mask,
   and keep a component iff the **median** ΔE of its pixels to the line
   colour ≤ 30 (so a brown line keeps its anti-aliased edge, a red line is
   rejected whole). Coverage must stay ≥ 90 %; the synthetic coverage test's
   second half (fills cover < 20 % of the ink mask) must pass.
4. **Owner rule**: qualifying = share ≥ 1 % of the owned pixels (no px
   floor); tile ascending; largest takes the remainder; the "< 400 px² goes
   whole" rule for tiny regions stays. `quality_redesign` must pass unchanged.

Acceptance as T9c's, plus: at 4× every pupil is one whole dark fill and
every brow is whole; `_under.png` shows no hatch on skin.

## T9e — masses by compactness (brows, pupils, hair stay fills); the hand loops

T9d review: pupils whole, hatch gone, `quality_redesign` green, coverage
94–98 %. Brows still thin/torn: a 12 px brow has radius ~6 < `max_line_radius`
9.65, so it is classed as a thin line, stroked and under-painted away; the
segmenter's "Eyebrow"/"Mouth" parts are hair/temple/jaw blobs, so part-mask
protection cannot save them. Guddu's and Woman1's hands show a scribbled ink
loop between thumb and fingers.

1. **Line vs mass by compactness, not radius alone.** For each 8-connected
   component of the candidate ink mask compute `area / skeleton_length`
   (skeleton from the existing thinning; for a component with no skeleton
   use `area / max(bbox side)`). It is a **mass** when that ratio
   > `2.5 × line_width_typical`, where `line_width_typical` = the median
   over all components of `2 × median EDT radius` (the artist's pen width,
   ~2–3 px here); else a **line** even where it thickens locally (so a
   pressed stroke keeps its continuity). Masses are removed from the ink
   layer (and from the under-paint set) and stay fills — brows, pupils,
   hair, shoes, belt buckle. Keep `max_line_radius` only as an upper bound
   for the skeleton's `D` step. Test: a 12×60 bar is a mass; a 3 px line with
   a 6 px-thick 20 px stretch is one line; a 10 px disc is a mass.
2. **Protected region** = masses inside the head owner part dilated 2 px,
   in addition to the eye-part pixels (T9d). Drop the Eyebrow/Mouth part
   masks from the protection (wrong parts).
3. **Hand loops**: look at `rigs_t9d/Guddu_zoom.png` (hand) and the ink layer
   there; if the loop is a skeleton junction artefact where the thumb line
   meets the fingers, prefer the straightest continuation (trace_lines step
   H) and drop whiskers < `min_line_length`; if it is the matte-hole fill
   creating a false outline, exclude holes < 400 px² from `filled_matte`'s
   effect on the ink pass. Report which.
4. Acceptance: Raju brows whole and dark at 4×; pupils whole; small
   features ≥ 80 % on all three; coverage ≥ 90 %; Raju ΔE ≤ 2.0; no loop
   scribble on Guddu's hand; body outlines continuous.

## T10 — full degree of freedom: wrists, ankles, brows as their own bones

User, 2026-10-06: a given character must be broken into hands, arms, wrists,
palms, upper/lower torso, thighs, shins, feet (both sides), head, and in the
head mouth, eye whites, eyeballs/pupils, left and right eyebrows — "required
for properly rigging and full degree of freedom".

**Today:** parts exist for Chest, Hips, Head, upper arm / forearm / hand ×2,
thigh / shin / foot ×2, Left/Right Eye, Left/Right Eyebrow, Mouth
(`part_masks`, ~654–760). But `RigPattern::biped()` has 11 slots, and
`map_part_to_biped_slot` (~3389) folds hand+wrist into the Elbow slot, foot+
ankle into the Knee slot and brows into Head, so hands, feet and brows ride
rigidly and cannot move on their own. Eyes: white, pupil and lid are their own
symbols (T4) with blink and look-at; the mouth is its own symbol.

**Design (append, never renumber):** a new `RigPattern::biped_full()` in
buzz-rig = `biped()`'s 11 slots **plus** `11 Wrist L` (parent Elbow L),
`12 Wrist R`, `13 Ankle L` (parent Knee L), `14 Ankle R`, `15 Brow L`
(parent Head), `16 Brow R`. The first 11 indices stay identical, so
`buzz_act` performances, lip sync, `eye_variants`, `relax_arms` (bones 3/5),
`bone.<n>.angle` and the standard pose library (applied "as far as it goes")
keep working; `biped()` itself is untouched (the built-in puppet and the
director keep their 11-bone skeleton).

1. `assemble_character` uses `biped_full()`; `biped_joints_from_skeleton`
   adds `(Wrist L joint → Hand L joint)`, `(Ankle L → Foot L)`, and for each
   brow `(inner end → outer end)` of the brow landmarks (fallback: the brow
   part's bbox left/right at its centre line); `map_part_to_biped_slot`:
   hand/wrist/palm/finger → 11/12, foot/ankle/toe → 13/14, brow → 15/16.
2. `part_masks`: the hand mask is cut at the wrist line (perpendicular to the
   forearm at the Wrist joint) and the foot at the ankle line, so the forearm
   and shin parts stop there (today the capsules overlap; the owner map
   decides — make the wrist/ankle the decider). Brow masks as today.
3. Caps/hidden fills for the new joints as for every other bone (T3 code is
   generic over `skel.bone(name)`: make sure `skeleton()` has "Wrist L/R"
   and "Ankle L/R" bones — it has Hand/Foot bones from wrist/ankle already
   (~255–264), so name them consistently).
4. Rigging panel: the slot list for a `biped_full` rig shows 17 slots
   (`pattern` name saved on the rig, `rig_panel.rs` reads `pattern.slots`).
5. Tests: a synthetic character assembles to 17 bones with the first 11
   equal to `biped()`'s names/parents; rotating Wrist L moves the hand part
   and not the forearm; rotating Brow L moves only the brow shapes; the
   standard 11-angle poses still apply; `buzz-act` tests untouched and green;
   `series_setup` rebuild of Raju gives a rig whose Cast asset lists 17 bones.

Not in scope: fingers, a neck bone (would re-parent the head and change
index 2's meaning), 3/4-view-specific parts.

## T11 — the brow part is the drawn brow; stray fragments

T10 made Brow L/R bones, but the segmenter's "Left/Right Eyebrow" part is a
100×118 px patch of forehead/hair around the landmarks (Raju), so the bone
would turn skin, not the brow. Also from the T9e rebuild: a black rectangle
floating right of Lalten's torso (front and back), two stray marks beside
Woman1 3-4's head.

1. **Brow part from the ink masses.** After `ink_components` (T9e), the brow
   part's mask = the dark **mass** components whose centroid lies inside the
   brow landmark box padded 60 % (and above the eye box), dilated 1 px; the
   patch pixels the segmenter gave the brow revert to the Head part. If no
   mass qualifies, the brow part is empty (no bone artwork; the bone still
   exists). Same for both sides. Test: synthetic head with a 12×60 brow bar
   above an eye ring → the Brow L part's shapes are exactly the bar (area
   within 5 %) and the head keeps the rest.
2. **Stray fragments.** A region or ink shape assigned to a part by the
   "nearest bone" rule (unowned pixels) must also be **near**: if its
   centroid is farther than `1.5 × limb radius` from the nearest bone
   segment, and it is not touching any owned pixel, drop it (it is a mark in
   the source outside the figure, or a segmenter artefact). Log how many
   were dropped. Check Lalten's rectangle: find what produces it (a prop? an
   unowned region? the `fill_thin_slits`?) by rendering the part that owns it
   (`BUZZ_TRACE_PROTECTED_PNG`-style debug) and fix the cause, not the symptom,
   if it is a bug.
3. Acceptance: Raju `rig_vs_source` unchanged within noise; a `biped_full`
   test that rotates Brow L moves shapes whose union bbox lies within the
   brow landmark box padded 60 %; Lalten (`series_setup --only Lalten
   --rigs-only --no-cast` into scratch) shows no floating rectangle; Woman1
   3-4 no stray marks.

## T8 — no gaps between clipped pieces; matte holes; contrast specks kept

Found reviewing T7 (`scratchpad/woman1_torso_zoom.png`, `guddu_head_zoom.png`):

1. **Clipped pieces must tile the region.** A straddling region is clipped
   against each qualifying part's *own* simplified boundary; two neighbouring
   boundaries do not coincide after independent simplification, so wedges of
   the region fall between the pieces and show the background (Woman1's
   waist). Fix: order the qualifying owners by share (largest first);
   `piece_1 = region ∩ B_1`; `piece_i = (region ∩ B_i) − (piece_1 ∪ … ∪
   piece_{i−1})`; the **last** owner takes `region − (all previous pieces)`
   so the union of pieces is exactly the region (test: areas sum to the
   region within 0.5 %, and no point of the region is uncovered — rasterise
   and check).
2. **Fill enclosed holes in the matte** before building `visible_rgba`:
   connected components of non-matte pixels that do not touch the picture
   border become matte (Guddu's hair highlight the segmenter left out).
3. **Specks (T7 follow-up):** the host region's colour comes from its own
   flooded cells only (merged cells excluded from the interior mean); a
   merged speck whose own mean colour is ΔE > 20 from its host and whose
   area is ≥ 6 px² at source resolution is also emitted as its own shape on
   top (drawn after the host, since larger regions come first), so small
   features survive and the hole stays filled. Smaller or low-contrast
   specks merge silently.

Acceptance: `rig_vs_source` Woman1 mean ΔE ≤ 3.5 and small features ≥ 85 %,
Guddu small features ≥ 88 %, Raju not worse than ΔE 1.85 / 89 %; no white
wedges or holes by eye on the three sheets at 2×.

## T5 — measurement: `examples/rig_vs_source.rs`

`cargo run --release -p buzz-app --example rig_vs_source -- <rig.buzz> <source.png> <out_dir>`
loads the rig, renders the armature at rest headlessly at the source's size
(place the rig so its bounds match the source's matte bounds), writes
`source.png | rig.png | diff.png` side by side and prints the `trace_png`
metrics (mean ΔE over the matte, edge distance mean/p95, small-feature
recall) plus shape count and the rig's `.buzz` size. Reuse the metric code
from `trace_png.rs` (move it into a small shared module under
`crates/buzz-app/examples/common/` or `buzz_scene::trace::metrics`).

## T6 — rebuild and review

`series_setup --replace` for the 12 rigs, `episode_build` for the episode
(see `ep1/buzz/README.md` for the commands), then `rig_vs_source` for Raju,
Guddu and Woman1 (front). Opus review: the sheets by eye at 1× and 2×, and
the numbers.

## Acceptance (per rig, Raju / Guddu / Woman1 front)

- mean ΔE ≤ 3.0; edge distance p95 ≤ 2 px; small features ≥ 85 %;
- shape count ≤ 3000 and trace ≤ 3 s at 1086×1448;
- by eye: plaid, bag, pens present; the eyes are the drawn eyes; no dashed
  outlines; no stair-steps at 2×; nothing floating outside the body;
- every rig part still poses (bend each shoulder/elbow 40° in a test: no
  shape leaves its part's bone further than the cap radius);
- `cargo test --release -p buzz-ai` and `-p buzz-app --test trace_bitmap
  --test break_apart` green; the ai_agent `ai_character_from_image` command
  unchanged in signature.

Not in scope: side views, new mouth charts, LaMa.
