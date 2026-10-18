# Trace spill, limb outpainting, and drawing on traced parts

User, 2026-10-08: "when the tracing is done I see some colour, white or something,
still seen outside the outlines and it feels odd, as if it was cut from somewhere in
a rough way. Make sure no colour or no part spills outside the outlines. It must feel
as if someone has drawn it. When this is perfected I want outpainting: when a full
image of a character is traced I want to cut the arms and legs out of it, but they
will not be complete, so I want them extended, either I draw or you outpaint."

Order: **Stage A (spill) first and accepted, then Stage B (outpaint), then Stage C
(drawing on parts).** Every step is mandatory; update the Status table after each.

## Status

| Step | State | Date | Evidence |
|---|---|---|---|
| D0 diagnosis: where the spill comes from, baseline numbers | done (Fable) | 2026-10-08 | below |
| A1 fills stop at the outer ink's centreline (`trace_colour.rs`, both routes) | implemented, not reviewed | 2026-10-08 | exterior = `!opaque` or (`region == skip` and not ink), per Fable's decision. Mass cores are exempt from the gate (otherwise hair and shoe mass fills would be cut in half). Distances use `edt` on a grid padded by 32 px, so the picture border never counts as exterior, ink or interior. On both routes no fill lies past centreline + 0.5. Fill reaches 41.19 px on transparency and 41.74 px on white; the parity test allows up to 1 px between them, because `grown_ink` grows only into opaque pixels, so on transparency the split sits about 0.5 px inside the ring centre. `cargo test --release -p buzz-scene`: 532 passed, 0 failed. Outputs at reach 1 / fringe 0 are byte-identical to before (9 pictures) |
| A1+A2 Fable review | **accepted** | 2026-10-08 | Spill on magenta (`spill.py`, before → after): Raju 7919 → 2379 px, beyond 1 px 3047 → 680 (all of it the one hand rectangle, see A3, plus ≤ 11 px anti-alias specks); Guddu 7175 → 1802, beyond 1 px 2260 → 284 (one 96 px rectangle, A3); Woman1 7201 → 1697, beyond 1 px 2370 → 117, max 3.0 px, largest blob 13 px. 6× arm-top crop: the skin sliver above the line is gone (`scratch a_raju_armtop_6x.png`). Ink coverage 98.46 / 98.17 / 97.51 %, edge p95 2.0 / 1.0 / 1.0 px (unchanged). Dialog route unchanged (2355 px, all ≤ 1 px). buzz-scene 532/532; buzz-ai 23/23 fidelity after one probe moved from the outline's outer half to its inner half (`dark_fills_do_not_sit_on_the_ink_and_a_light_fill_runs_under_it`: x 81.5 → 82.5, by design). |
| A2 the matte's 1 px anti-alias fringe outside the ink is exterior, not a fill (`trace_colour.rs`) | done, reviewed with A1 | 2026-10-08 | `the_segmenters_light_fringe_is_not_painted` passes (grey ring unpainted, red under the ink). `fringe_px` slider is in place and `tuning_section_lists_every_field` passes |
| A3 the stray rectangles (under Raju's right shoe before A1/A2; under Raju's right hand and beside Guddu's right sleeve after): find them with per-part renders, fix the cause | **done** (rectangles fixed), not reviewed; spill thresholds deferred to Fable | 2026-10-08 | **Cause:** the straddler clip (`buzz_geom::boolean` in `trace_character_whole_with`) left zero-area sliver subpaths in clipped pieces, and the renderer's seam seal stroked them: vello's stroke of a subpath that is one point in f32 leaks its winding along its 16 px tile row out to the path's bounds. **Fixes:** (1) producer: clipped pieces drop subpaths thinner than 0.25 px mean (`without_sliver_subpaths`, `character.rs`); (2) renderer: the seal skips subpaths thinner than 1e-3 device px (`sliver_subpaths`, `buzz-render`). Evidence: not the T3 hidden fill (Hand R has no hidden pixels). `rig_vs_source --parts`: all 602 px beyond 1 px are in `hand_r`; `--shapes hand_r`: all in `hand_r_fill_0`, the skin piece the straddler clip (`buzz_geom::boolean` in `trace_character_whole_with`) cut from the Elbow R / Hand R region: MoveTo 33, LineTo 487, ClosePath 33, all closed, none under 3 points, no NaN/inf, but only 1 real outline (404 points, 4636 px²) and **32 zero-area subpaths** (mean thickness `2·|A|/P` ≤ 0.024 px). One of them alone makes the band: `M92.41563,432.16934 L92.41564,432.16934 L92.41564,432.16934 Z` (3 points 3e-6 px apart, one point in f32). The renderer's seam seal strokes every subpath; vello's stroke of a direction-less subpath leaks its winding along the 16 px tile row (row 27 = y 432–447, tiles 6–11 = x 96–191) out to the path's bounds (x 191); the debug rasteriser has no seal, hence no band. Isolated in a test: outline + that subpath → 192 px painted outside the outline; seal off (alpha 0.99) or the subpath removed → 0. Guddu: `shoulder_l_fill_1` (cream piece: outline, an 8.7 px-thick hole, **66 slivers** ≤ 0.095 px), band in tile row 23 out to x 927. Fix, both ends: (1) producer — every clipped piece drops subpaths thinner than 0.25 px mean (`without_sliver_subpaths` in `character.rs`; test `a_clipped_piece_drops_the_slivers_the_boolean_left`); (2) renderer — the seal skips subpaths under 1e-3 device px (`sliver_subpaths` in `buzz-render`; tests `a_seal_skips_the_subpaths_that_enclose_nothing` and buzz-export `degenerate_subpath_seal`, which failed with 192 px before and passes after). After (`rig_vs_source`, magenta): Raju spill 2379 → 1689 px, beyond 1 px 680 → 68, max 14.32 → 2.83 px; Guddu 1802 → 1692, 284 → 185, 10.3 → 2.83; ink coverage 98.46 / 98.17 % and mean ΔE 2.23 / 10.34 unchanged; Raju edge p95 2.0 → 1.41; shapes 391 → 387 / 582 → 578 (pieces that were only slivers). **Residual (not A3):** 30 / 124 blobs ≤ 22 px of dark-ink anti-alias over magenta (e.g. [173,5,173]) — Raju at the Hand R cap's exposed arc (x 187–202, y 388–390) and the face; Guddu x 185–203 y 315 and the head (per part: `head` 105, `hand_r_caps` 27, `hand_l_caps` 25 beyond 1 px). The A5 targets (≤ 20 beyond 1 px, max ≤ 2.0) are not met yet; deferred to Fable, who will inspect these specks and open a separate step if they are real. buzz-scene 532/532, buzz-ai all green (lib 76/76, trace_fidelity 23/23), buzz-render lib 106/106 |
| A4 diagnostics: `rig_vs_source --parts` + `spill_px` / `spill_max_px` metrics, `BUZZ_EXAMPLE_BG` documented | done, not reviewed | 2026-10-08 | `--parts`: each armature part alone (every other dropped from the rig, same framing) on magenta → `<name>_parts/<part>.png` (`hand_r`, `hand_r_caps`, `face_left_eye`, …) and its spill on stderr. `--shapes <part>`: each fill / ink / cap of that part alone → `<name>_shapes/<part>_<class>_<k>.png` (numbered as `BUZZ_TRACE_SHAPES_LOG`), its spill, its path elements (counts, subpaths under 3 distinct points, unclosed, non-finite) and the path as SVG (`.svg.txt`). The JSON line gains `spill_px`, `spill_beyond_1px`, `spill_max_px` from a separate magenta render (whatever `BUZZ_EXAMPLE_BG`), computed as `spill.py` (figure: > 20 from pixel (2,2), closing 2 with the 4-cross, holes filled; exact EDT): Raju before the A3 fix 2379 / 680 / 14.32 = `spill.py`'s 2379 / 680 / 14.3. Module docs cover `BUZZ_EXAMPLE_BG` and the metric (`trace_png` has no spill metric, its docs untouched) |
| A3b joint caps inset (Fable, direct) | done | 2026-10-08 | After A3 the residual blobs sat at the wrist/ankle caps: `cap_ring` walked the owner map to the silhouette's last owned pixel (the outline's outer half, since the matte ends on the ink skirt), so the cap disc showed as a 1 px sliver of cap colour past the line and the stroked exposed arc poked half its width out. `cap_ring` gained `inset`: disc 1.0 px, arc 1.0 + uw/2 (`character.rs` ~2260, ~4600, ~4612). Raju beyond-1 px 68 → 37, Guddu 185 → 140 (max 2.83 → 2.0), Woman1 117 → 73. |
| A5 Fable review: spill gate on Raju / Guddu / Woman1, 4× sheets on magenta, suites green | **accepted** | 2026-10-08 | Final (`rig_vs_source` JSON = `spill.py`): Raju spill 1627 px, beyond 1 px 37, largest blob 9 px, max 2.83 px; Guddu 1617 / 140 / 4 px / 2.0 px; Woman1 1632 / 73 / 12 px / 3.0 px. Baseline was 7919 / 3047 / 2039 px / 36 px (Raju). What remains is the ink line's own anti-aliasing: the vector line is centred on the source's outline footprint, so its outer half plus anti-alias lies up to ~2 px past `spill.py`'s figure mask, and the two near-white specks (Raju x 572 y 59, Woman1 x 468 y 184) are hair highlights the figure mask excludes. **Gate as accepted:** largest blob ≤ 12 px, `spill_max_px` ≤ 3.0, `spill_beyond_1px` ≤ 150 — the plan's original "0 beyond 1 px" was unattainable for a centred anti-aliased line and is replaced by this. 4× / 6× crops (scratch `e_raju_{hand,wrist,feet}.png`): hand clean, no sliver above the arm's top line, feet without the slab; the shoe's fragmented mass ink is unchanged and out of scope (noted in D0). mean ΔE 2.23 / 10.34 (Guddu/Woman1 measured on magenta, not comparable to white), ink coverage 98.46 / 98.17 / 97.51 %, edge p95 1.41 / 1.0 / 1.0. Suites: buzz-scene 532/532, buzz-ai all green (lib 76, fidelity 23), buzz-render lib 106/106, buzz-export seal test 1/1. |
| B1 `outpaint_limb`: cross-section extrusion along the bone (replaces capsule + flat fill for limbs) | implemented, not reviewed | 2026-10-08 | New `crates/buzz-ai/src/outpaint.rs`: `outpaint_limb` (1 px stations, 5-station median profile; cut at the head when the visible pixels start at `t_cut > 0`, else at the tail; tube from `t_cut` to the joint centred on the profile's centre at the cut, width extrapolated with the lower-median slope of the 8 stations next to the cut, clamped 0.7–1.3×; round end of radius `r_cap` centred on the tube's centre line at the joint (= the head joint when the bone runs through the limb's middle); colour = the cross-section at station `t_cut + 2`, `s` rescaled by the width ratio, so outline rows, stripes and shading continue and the end gets no ink across it), painted only on `tube ∧ allowed` (`allowed` = today's hole rule: matte, owner paints over, not visible). `nearest_visible_fill` (8-connected first-seed layers, as `under_paint`) for Chest / Hips / Head holes. `complete_parts_impl` uses them whenever LaMa did not fill (LaMa path unchanged); a limb with no visible pixel keeps the capsule hole in `dom_color`. `CompletedPart.extruded: Vec<bool>` (`#[serde(skip)]`, empty when no tube). Tests: `outpaint::tests::a_cut_limb_continues_its_stripes_and_outline_to_the_joint` (stripes + 3 px outline rows equal the station at the cut to the joint, matte slot and outside-tube pixels transparent, nothing past head − r_cap, round tip without outline), `a_part_cut_at_its_tail_extrudes_towards_the_tail`, `a_non_limb_hole_takes_the_nearest_visible_colour`. buzz-ai lib 79/79 |
| B2 hidden completion traced as Ink + colour behind the part, tube pixels exempt from the reach gate | implemented, not reviewed | 2026-10-08 | T3 on the Ink + colour path (`use_ink_colour`; the T9e and `outline_strokes` paths keep the old Outline flat fill: they have no Ink + colour trace to share tuning with, and `outline_strokes` has no ink layer): hidden picture = `part.rgba` on the hidden pixels + a 2 px (Chebyshev) overlap into the part's owned pixels, traced with `options.trace` as `InkAndColour`, `drop_background: true`; fills spliced at the front of `per_part_shapes[id]`, ink at the front of `per_part_ink[id]`; `reach` gate skipped for `extruded` pixels; face parts and caps untouched. Traced on a window (hidden bbox + 40 px margin, shapes translated back), not the whole canvas: whole-canvas traces took Raju's pipeline from 11.8 s to 41.6 s, the window 10.4 s; the two posed renders differ in 6 px. Tests: new `a_limbs_hidden_completion_is_ink_and_colour_behind_its_visible_shapes_and_inside_the_matte` (`trace_fidelity.rs`: dark ink and an arm-colour fill at the front, all left of the cut + overlap, reaching the tube's far end, 0 px outside the matte); `a_covered_part_gets_a_hidden_fill_at_index_zero_and_a_visible_part_none` moved to the B2 contract (k ≥ 1 front fills, the strip plus the 2 px overlap, fills + ink cover ≥ 95 % of the hidden pixels within reach). trace_fidelity 24/24, every buzz-ai suite 0 failed. Raju rest (`rig_vs_source`, magenta): spill 1614 px, beyond 1 px 34 (was 37), max 2.83, ink coverage 98.46 %, edge p95 1.41, mean ΔE 2.23 (all unchanged or better); shapes 387 → 429, ink shapes 33 → 48; vs the A5 render 169 px differ by > 24. `rig_vs_source --pose <bone>=<deg>,...` added (`<name>_posed.png`); Raju `Shoulder_L=70,Shoulder_R=-70,Thigh_L=30,Thigh_R=-30` (scratch `b12b/raju_posed.png`): the arm behind the chest shows its sleeve outline and plaid continuing to a round top, with a few-px magenta sliver left between it and the collar; the arm in front of the chest ends in its segmentation cut (nothing paints over it there, so nothing to complete); the far leg, hidden behind the near leg almost along its whole length in this profile T-pose, shows as flat ragged pieces when swung back (no cross-section to extrude); ankles not turned. Not yet compared with a pre-B posed render |
| B3 editor command **Extend part** (selected rig part or symbol) + agent verb | **X1 implemented, not reviewed** (spec § Round 2026-10-14, X1) | 2026-10-14 | `Command::ExtendPart` (`extend_part`, "Extend Part Under Neighbours"; `buzz-ui/src/command.rs`: id, label, no shortcut, `needs_selection`, palette; Modify ▸ "Rig" block after Bitmap in `panels.rs`; agent `Route::Run` in `agent.rs`). Handled in `Editor::run` → `Editor::extend_part` (`editor.rs`), synchronous, no App interception. Per selected instance on the current timeline (stage or a symbol open in place): joint = `pivot_of` through the layer's `inherited_transform`; occluders = visible objects later in paint order (`drawable_at[_by_depth]`, tween-resolved via `resolved_at_rigged`, mask/guide layers skipped) overlapping the part's bounds grown 25 %; region = part ∪ occluders ∪ joint disc, +10 %, 2 px/unit capped at 2048 px. Pixels from the new free fn `render_only`: a disposable scene whose stage is a copy of the current timeline with every other object hidden (masks kept), camera off, the part symbol's own "Extension" layer removed, rendered by a `buzz_export::Exporter` made once per command (straight RGBA, exporter un-premultiplies); one render of the part, one of the occluders (alpha only). `outpaint_limb` unchanged; trace = extruded + part pixels within 2 px Chebyshev, `ink_colour().with_saved_tuning()`, `drop_background`; shapes mapped pixel → timeline → part space; one undo step "Extend Part" writes/replaces a bottom "Extension" layer keyed at the shown symbol frame over the symbol's span. Status messages per spec (not a symbol, armature, nothing above, joint). Proof `examples/extend_part_check.rs` (scratch `x1/`): status "Extended 1 part(s)", 17 shapes on Extension; rest diff with torso visible 0 px (> 24); 18 649 px added with torso hidden, 0 outside the torso's footprint; rotated 60° about the joint 1535 px differ (extension shows below the torso); undo removes the layer, 0 px vs before. Seen in `hidden_after.png`/`rotated_after.png`: stripes and both outline rows continue to a round end, but a 1 px light seam line remains at the cut and the extension's base colour is a little lighter than the arm's (trace quantisation) — for review. Only changed test assertion: `every_unit_command_round_trips_through_its_id` count 196 → 197. buzz-ui lib 375 passed (`--test-threads=1`); buzz-app lib filtered (agent/command/menu/shortcut) 35 passed incl. `every_menu_command_runs_and_the_window_still_draws` |
| B4 Fable review of B1/B2 (posed Raju: `rig_vs_source --pose Shoulder_L=70,Shoulder_R=-70,Thigh_L=30,Thigh_R=-30 --parts`) | reviewed — **arms accepted, legs not**; follow-ups B1b–B1d below | 2026-10-08 | Rest render unchanged within noise (spill beyond 1 px 34, ink 98.46 %, edge p95 1.41). Posed (scratch `b12b/raju_posed.png`, crops `b_posed_shoulders_3x.png`, `b_posed_hip_2x.png`): both arms swing down with their sleeve outline and plaid continuing to a round top; a few-px magenta notch between the far arm's top and the collar (the segmenter gave the shoulder top to the far arm, which is *behind* the chest, so the chest has no "hidden" completion there). Hips: the near thigh's top is the segmenter's **saw-tooth Hips/Thigh boundary** (per-part rest renders `rvs_f/raju_parts/thigh_r.png`, `f_parts_hip_rest.png`), so B1 found no cut to extrude (`t_cut ≈ 0`) and the jagged top rotates out from behind the hips as a straight-edged dark wedge; the far thigh/shin are slivers and fragments (hidden behind the near leg along their whole length in this profile T-pose: no cross-section exists), shown as ragged pieces when swung back. These are exactly the "cut roughly" look the user rejects, so B is not done. |
| B1b hidden counterpart: a limb with (almost) no visible pixels borrows its L/R counterpart's completed picture | implemented, not reviewed | 2026-10-08 | **Spec as implemented** (`complete_parts_impl`, `outpaint::borrows_counterpart` / `borrow_counterpart`): a limb part (Thigh/Knee/Ankle or Foot, Shoulder/Elbow/Wrist or Hand) owning < 20 % of its L/R counterpart's segmenter pixels borrows. Coordinator decision (a): before anything reads the paint order, the WHOLE chain of that side gets depth = the counterpart chain's lowest depth − 1 (`CompletedPart.depth`, which `TracedPart.depth` and `assemble_character`'s layer order read); if both sides borrow (different slots) the side owning fewer pixels goes behind. After all parts are completed, the borrower's rgba = its own visible pixels (on top) + the counterpart's completed picture mapped by the similarity transform head→head, tail→tail (no mirror, nearest pixel) on every pixel it may paint (matte ∧ the owner paints over it, today's hole rule), so the rest render is unchanged. Borrowed pixels are marked in `extruded` (T3/B2 trace them with no reach gate: the hidden set is everything the near limb covers); `CompletedPart.borrowed_from: Option<String>` (`serde(default)`, skipped when `None`). `BUZZ_OUTPAINT_LOG=1` prints the borrowers and every joint cut. Test `outpaint::tests::a_hidden_limb_borrows_its_counterpart` (profile pair: far Thigh L owns 2 of 31 columns → `borrowed_from == Some("Thigh R")`, depth 60 → 49, its picture = the near leg's picture 4 px over where hidden, its sliver on top; front pair: nothing borrowed, depths unchanged). **Real runs:** Raju: only **Foot L** borrows (2364 px vs 16044); Thigh L (24 %) and Knee L (22 %) are above the 20 % trigger, so the far leg is still slivers when swung back (posed `rvs` scratch `b1c/raju_posed.png`: far thigh/shin = thin dark fragments, far foot = a whole borrowed shoe). Guddu (front view): `borrowed_from` none. Pujari test fixture: Wrist L (16 px, the segmenter gave the raised hand to the forearm) and Thigh R (581 px, the folded-up leg) borrow, which sends the whole left arm from depth 80 to 39 (behind chest and right arm) — a trigger from segmentation, not occlusion; the rest picture is unchanged (`rig_quality` invariant). |
| B1c clean cuts at the joints: a limb's boundary with its parent is a straight cut perpendicular to the bone, the limb extruded from that cut to the joint | implemented, not reviewed | 2026-10-08 | **Spec as implemented** (coordinator decision (c), by paint order; `outpaint::measure_joint_cut` / `apply_joint_cut`, called from `complete_parts_impl` after B1b's depths, before the owner ranks, so `CompletedParts` and `composite_completed`'s owner map carry the cut; `part_masks` is untouched, nothing downstream reads it). At all 12 joints (Chest→Shoulder, Shoulder→Elbow, Elbow→Wrist/Hand, Hips→Thigh, Thigh→Knee, Knee→Ankle/Foot; the wizard names the hand/foot bones Hand/Foot, `skeleton()` Wrist/Ankle, both accepted) OVER = the part that paints later by (depth, list position). Cut ⟂ the child bone at `t_cut` = first station from the joint where OVER's visible profile in the band reaches ≥ 85 % of the median width of the next 10 stations, clamped 0.1–0.4 × the child bone, rounded. **Band** (the limb's profile width) = per station the matte's run through the limb's centre line, clipped to 1.25 × the limb's own half-width (median extent of the child part's pixels at 0.4–0.6 of its bone; the matte's run there took pujari's whole kurta into the arm). **Re-partition** within the band: child pixels at −len ≤ t < t_cut → parent; parent pixels at t_cut ≤ t ≤ len and nearer the child bone than the parent bone → child; two children of one parent (thighs on Hips, arms on Chest) contest a parent pixel: a chain B1b moved behind loses, else the nearer bone axis. **Deviations from the decision's wording, needed for its stated outcome:** (1) a parent OVER (Hips over Thigh, Chest over Shoulder R) gives up its pixels *past* the cut (the lobe), not those between joint and cut (that left the hips a hole at the joint and the lobe beyond the cut); (2) child pixels behind the joint move too (−len, not 0): with 0, pujari's bent elbow kept a forearm island below the joint; at the ankle the heel therefore goes with the shin (one straight cut through the shoe) rather than heel | strip | toe; (3) a limb joint bent > 90° (child bone · parent bone < 0: pujari's raised forearm, the fixture's folded shin) gets no cut, the parts lie side by side; (4) UNDER parents (thigh under the knee) own the moved strip past the joint and get no tail tube (their visible pixels already reach past it); a child OVER keeps a bare straight end (nothing paints over it, so no tube may be painted; the joint cap sits behind). **B1 tube** now tapers linearly from hw0 at the cut to `r_end = min(hw0, dist(joint on the tube's centre line → nearest non-matte) − 1)` at the joint, round end of radius `r_end` (the 0.7–1.3 slope extrapolation is gone; `outpaint_limb` takes the matte). Tests: `outpaint::tests::a_saw_tooth_joint_boundary_becomes_a_straight_cut_and_a_round_end` (saw-tooth 66–86 px under a parent block → t_cut 12, every limb row past it the limb's, none short of it, tube to the joint, r_end 6 = 7 − 1 < hw0 13, taper checked at both ends, round tip); B1's first test moved its matte slot 60→70 so the round end keeps r = hw0. Two pujari contracts restated for B1c (pixels change owner by design): `complete_parts_on_ground_truth_fixture` (no owned pixel is lost from the picture, was: no part loses a pixel) and `completion_never_fills_a_pixel_owned_by_a_part_it_would_paint_over` (the topmost completed part at every owned pixel shows the source pixel, by the completed depths). buzz-ai: lib 81/81, trace_fidelity 24/24, all 26 suites 189 passed, 0 failed. **Raju rest** (`rig_vs_source`, magenta): spill_beyond_1px 34 → 26, spill_max_px 2.83 → 2.24, ink_coverage 98.46 → 98.59 %, edge_p95 1.41 = 1.41, mean ΔE 2.22; cuts: hips t_cut 31/32 px (1192 / 6648 px moved), knees 30/31 px, feet 28/9 px, shoulders 13/12 px. **Guddu rest:** spill_beyond_1px 129, max 2.0, edge_p95 1.0, ink_coverage 97.73 % (A5: 98.17; not isolated whether B2 or B1c). **Raju posed** (`Shoulder_L=70,Shoulder_R=-70,Thigh_L=30,Thigh_R=-30,Knee_L=20,Knee_R=-20`, scratch `b1c/raju_posed.png`, `b1c_posed_hip_knee_shoulder.png`): arms and shoulders clean (sleeve continues, no notch visible at this scale); near thigh leaves the hips with no saw-tooth top, but a flat straight-edged dark piece still sticks out behind the hips and the far thigh shows as a thin dark strip; near knee = a straight-cut step (the cut-out look) with a few small spikes at the back of the knee; far thigh/shin still fragments (B1b not triggered for them). **Not done:** the user's "no straight-cut edge, no fragments" on the legs. |
| B1d chest/hips completion under a limb that is *behind* them | implemented (P8-B, Opus), not reviewed | 2026-10-14 | Spec: a non-limb part may complete a hidden pixel owned by a limb behind it when the pixel lies inside the part's own capsule prior (the segmenter's box/capsule for that part); nearest-visible colour as B1. Closes the collar notch. **As implemented** (`outpaint::body_prior`, `outpaint::take_back_joint_side`, called in `complete_parts_impl` after the B1c cuts): for each joint where a body part (not a limb bone) paints over its child limb, the limb's pixels on the joint side of the cut (`t < t_cut`) inside the body part's prior (its completion capsule ∪ the 5-station-median span of its own rows along its bone) are relabelled to the body part. **Deviation:** they keep their source colours, not the nearest visible colour — the body part paints over the limb, so a flat colour there would repaint the rest picture (ink lines lost); with the source pixel the rest render is identical and the limb is completed under them. Raju: Hips take back 1525 px of Thigh L (the far leg's strip under the belt), Chest 0 px of Shoulder R (no magenta notch left after B1c; the far shoulder's top outline gap lies outside the chest's capsule, open). Proof `scratchpad/produce/p8b/s3/`. |
| B4b Fable re-review: posed Raju/Guddu/Woman1 at ±70° arms, ±30° thighs, 20° shins: no straight-cut edges, no fragments, rounded ends; rest spill gate still holds | P8-B items implemented (Opus), review todo | 2026-10-14 | P8-B (docs/PRODUCER_PLAN.md Status row 'P8-B', renders `scratchpad/produce/p8b/`): **B4b-2** the hip wedge was the far thigh's own 1978 px tongue (segmenter pixels inside the near trouser leg) and the matching hole in the near thigh; hips take back the far leg's strip under the belt (B1d). **B4b-3** `outpaint::round_over_end` (shin gets everything from the knee on + a half-disc of radius half the band width past it, source pixels), thigh extruded at both ends (`outpaint_limb_both_ends`), `remove_spikes` (4×4 opening) on thighs/shins: step and wedge gap at the knee gone. **B4b-1** `outpaint::drop_borrower_fragments`: a borrower's slivers (run across the bone < 4 px) and components < 15 % of the borrowed picture go to the counterpart (Raju: 7001 / 5930 / 2347 px of Thigh/Knee/Foot L): far leg one clean trouser leg, near leg without holes or comb. **B4b-4** owner Shoulder L: the pocket pens; `outpaint::return_foreign_objects` returns them to the chest. All relabels keep the source pixels on a part that paints over (or, B4b-4, a body part the limb then leaves uncovered), so the completed composite is pixel-identical at rest; the trace's per-part clip moves (Raju 1471 px > 24 vs before, shoe mass ink). Rest: Raju 39 / 2.24 / ink 98.50 %, Guddu 129 / 2.0 / 97.72 %, Woman1 17 / 3.61 / 96.00 % (3.61 pre-existing). Open: far-shoulder outline gap (round tube end + chest's straight cut, no ink), ~4 px tick on Raju's near sleeve, ~3 px kink at the back of the near knee, small red sari flap at Woman1's knee.  **B4c (front views, 2026-10-14):** Guddu's shirt-side holes are Hips-owned stripe regions the trace's straddler clip gives whole to the thighs (`thigh_l_fill_3`: 349 of 3180 px the thigh's) — fix needed in `trace_character_whole_with`/buzz-geom, not in completion (a flank take-back closed them but lost the hem outline at rest, ink −0.5 %). Shoulder tabs: cut forced at Chest→Shoulder when unmeasurable (`measure_joint_cut_with`) + shoulder `round_over_end` keeping the chest's capsule; left tabs rounded, a thin streak / speck remain. Woman1 hip notch: the thigh owns the sari's hip flank, Hips owns ~nothing; forcing Hips/Thigh cuts cost 0.9 % ink → not shipped.  **B4c round 3:** the holes' cause was `split_remainder_by_bone` (whole remainder to the nearest clamped bone: a shirt stripe above the thigh's head went to the thigh); fixed by majority ownership for loops past a bone's head, a 1.15× piece guard that splits by rasterised ownership, the flank take-back, and part-boundary simplification clamped to ≤ 4 px (the hips' 12 px tolerance had cut the hem line from its ink clip). Guddu torso holes 3868 → 1 px (rest ink 97.62 %); Woman1 forced Hips/Thigh cuts now hold (ink 96.08 %), hip notch gone.  **B4c shoulders (final round):** chest completion now covers the joint disc of an arm painting over it (`outpaint::joint_disc`, `body_discs`): Woman1's sleeve notches filled, Guddu's smaller; open: Guddu's right-shoulder chest tab and the rounded hump of the under-arm's root above the left shoulder (Guddu, Woman1); a root trim to the joint disc was tried and removed (ghost sleeve on the chest). |
| R1 (2026-10-14) **Reset to recommended** in the Trace Bitmap dialog and the AI character dialog: every setting (mode, bold, smoothness, colours, speck, tuning) back to `TraceOptions::ink_colour()` / `TraceCharacterOptions::default()`, ignoring the saved tuning file | implemented (planner, direct); `cargo check -p buzz-app` clean | 2026-10-14 | `trace_dialog::recommended_row` + `RECOMMENDED_NOTE`; the old **Reset** inside Tuning still resets only the tuning sliders |
| W1 (2026-10-14) AI ▸ Character from Image starts from the **picture selected on stage** (the AI cuts it into parts, completes the cut sections, traces with the recommended settings, layer rig) | implemented (planner, direct); `cargo check -p buzz-app` clean | 2026-10-14 | `Editor::preload_ai_character_from_selection`: the picture's file is written to `%TEMP%/buzzanimate-character-input/<name>.<ext>` and put in the wizard's path box |
| X1 review (planner, 2026-10-14) | **accepted after two fixes** | 2026-10-14 | Looked at `scratchpad/x1/{hidden,rotated}_after.png`: the extension worked, but there was a 1 px white seam at the cut and the colours were lifted (tan 240,200,150 → 252,210,157). **Cause:** the arm's stroke corners poke ~3 px past a hand-made cut, so `outpaint_limb` started the profile there: the tube ended 3 px short of the fill edge, and the colour station (`t_cut + 2`) was still left of it, so the samples were the anti-aliased edge column (alpha 243, un-premultiplied lighter). **Fix in `outpaint_limb`:** the tube runs, per row across it, up to that row's first visible pixel (≤ `ROW_REACH_PX` 12 past `t_cut`); colour from the first *full* cross-section (≥ `FULL_SECTION` 0.9 of the widest near the cut); samples prefer pixels with alpha ≥ 250. After (`scratchpad/x1dbg/`): no seam, tan exact, blue within ΔE ≈ 3 (tracer quantisation); rest diff 0 px, 0 px outside the torso, undo clean. AI route unchanged at rest: Raju `rig_vs_source` spill beyond 1 px 26, max 2.24, ink 98.59 %, edge p95 1.41, ΔE 2.22 (= B1c). buzz-ai 26/26 suites, buzz-ui 375/375. |
| B1b′ chain borrow (planner, 2026-10-14) | implemented + looked at; part of B4b | 2026-10-14 | Once one slot of a side borrows (< 20 %), that side's other slots borrow too under `CHAIN_BORROW_SHARE` 0.4 (`outpaint::chain_borrows`). Raju: Thigh L (8496 / 34774 px) and Knee L (5803 / 26390) now borrow with Foot L. Posed (`scratchpad/b4b2/raju_posed.png`, same pose as B1c): **the far leg is a whole trouser leg + shoe** (was fragments + a floating shoe). Rest unchanged (spill beyond 1 px 26, max 2.24). Front views never trigger. Still open for B4b: ragged slivers along the far thigh's top (its own visible sliver drawn over the borrowed picture), a dark wedge behind the hips (near thigh), a cut step + spikes at the near knee; small dark slash on the near sleeve top. |
| C1 headless proof: open character → open part in place → brush stroke lands in the part and survives posing | todo | | |
| C2 ghosted neighbours while a part is open in place (toggle), so the hidden continuation can be drawn | todo | | |
| C3 Fable review | todo | | |

## D0 — Diagnosis (measured 2026-10-08)

Tools: `rig_vs_source` and `trace_png` now take `BUZZ_EXAMPLE_BG=#rrggbb` (stage
colour; Fable added it today) so light fills outside the outline are visible. Spill
is measured by scratch `spill.py <source> <render> <prefix>`: *figure* = source pixels
that differ from the corner background by > 20, closed 2 px, holes filled; *spill* =
rendered non-background pixels outside the figure. Sources:
`B:\youtubeProjects\Originals\series\ep1\<raju|guddu|woman1>\...` (RGB, white
background, no alpha).

| route | character | spill px | beyond 1 px | blobs | largest blob | max dist |
|---|---|---|---|---|---|---|
| Trace dialog (`trace_png inkcolour`) | Raju | 2356 | 74 | 3 | 36 px | 5.1 px |
| Character wizard (`rig_vs_source`) | Raju | 7919 | 3047 | 841 | **2039 px** | **36.2 px** |
| Character wizard | Guddu | 7175 | 2260 | 950 | 145 px | 13.0 px |
| Character wizard | Woman1 | 7201 | 2370 | 1014 | 25 px | 3.6 px |

The dialog route is nearly clean (its spill is the ink's own anti-aliasing at ≤ 1 px).
The wizard route has **~1.5 px of spill per boundary pixel all along the silhouette**
(7000 px over a ~5000 px boundary) plus one slab. Three mechanisms:

1. **The fill under the outer ink runs to the ink's outer edge (and 0.25 px past
   it), but the vector ink is centred on the ink footprint.** In the wizard route the
   picture outside the matte is transparent, so the whole outer ink line is
   under-painted from the inside (`under_paint`: nearest opaque non-ink pixel = the
   figure), `cell_labels` gives every ink cell its donor's region, and `region_path`
   grows by `REGION_GROW_CELLS`. Rendered: a skin-coloured sliver *above* the black
   line on the top of Raju's arm (scratch `spill_wizard_armtop_6x.png`). In the
   dialog route the outer half of that line is donated by the white background
   region, which is dropped — hence no spill there.
2. **The segmenter matte ends ~1 px outside the ink, on the ink's anti-alias
   skirt.** Measured on the part-mask dumps (`ai_character --stage parts`): matte
   boundary → nearest ink median 1.0 px; the non-ink matte pixels reachable from
   outside without crossing ink are 0.68 / 0.98 / 0.90 px per boundary pixel
   (Raju / Guddu / Woman1) with luma median 187–200 — light grey. They are opaque,
   non-ink, and become fill (merged into the neighbour or their own thin region):
   the "white or something" the user sees.
3. **A slab under Raju's right shoe**: rendered block x 491–671, y 1438–1455, colour
   [70,54,45] (the shoe/shin dominant colour), straight bottom and left edges, 8 px
   below the lowest source pixel (row 1447). `BUZZ_TRACE_SHAPES_LOG=420,1390,680,1470`
   lists no trace-space shape whose bounds reach below row 1448, and the parts debug
   PNG shows nothing there, so it is **not in the trace**: something is drawn
   displaced at assembly or render time (a duplicate of a foot/shin fill a few px
   down-left, a cap, or a stroke). The posed Cast preview (`rigs/Raju.png`,
   2026-10-07) shows the same thing as a light-grey plate under the right shoe.
   Guddu's 145 px blob at x 123–198, y 304–320 ([216,133,96], skin) is a second
   case to check with the same tool.

Not spill, but seen on the same crops and noted for later: the shoe's ink is
fragmented (mass cores ≥ 600 px² drawn as fills with rims), and the trouser crease
line is missing. Out of scope here.

## Stage A — nothing outside the outline

### A1 Fills stop at the outer ink's centreline

`crates/buzz-scene/src/trace_colour.rs`, in `trace_ink_and_colour` after `skip` is
known (step 8) and before `cell_labels` (step 6+7):

- `exterior: Vec<bool>` (native) = `!opaque[i]` **or** `region[i] == skip` (the
  dropped background region, dialog route). When neither exists (no transparent
  pixel and `drop_background` off) nothing below applies.
- Two distance transforms on the native grid (`trace_lines::edt` or the existing
  helper used by `ink_window`): `d_ext[i]` = distance to the nearest exterior pixel,
  `d_int[i]` = distance to the nearest pixel that is opaque, non-ink and not
  exterior. Both are only needed on ink pixels within, say, 8 px of an exterior
  pixel; compute them on the whole grid if that is simpler (1.5 Mpx, cheap).
- New `ColourTuning::exterior_fill_reach: f32` (default **0.5**, range 0–1, serde
  default, hover text: "How far under an outline on the figure's edge the fill may
  reach: 0 stops at the line's inner edge, 0.5 at its centre, 1 at its outer
  edge."). An ink pixel keeps its donor region iff
  `d_int <= reach * (d_int + d_ext)`; otherwise its cells are `NONE`. At 1.0 this is
  today's behaviour exactly.
- Sub-pixel: in `cell_labels`, for ink pixels with `|d_int - reach*(d_int+d_ext)| <= 1`
  evaluate the inequality with the two distances bilinearly interpolated at the
  cell centre (the same sampling `sample_flat` uses), so the fill edge is smooth at
  `U×`, not stair-stepped per native pixel.
- Interior ink (a line between two fills) has a large `d_ext` and is unaffected.
- Also pass `exterior` into `region_path`'s growth: a grown cell that lands on an
  exterior cell must not be set (otherwise `REGION_GROW_CELLS` re-adds 0.25 px).
- Add the field to `tuning_section` (`crates/buzz-ui/src/trace_dialog.rs`, Colour
  group; the `tuning_section_lists_every_field` test counts fields from the
  serialised defaults, so it fails until the slider exists).

Test (buzz-scene, `trace_colour::tests`): a red disc outlined by a 3 px dark ring
on a **transparent** background, traced with `ink_colour()`; rasterise the fills
only (no ink shapes) with the crate's test rasteriser and assert no painted pixel
lies farther than 0.5 px outside the ring's centreline circle, and that the fill
still reaches the centreline (no gap under the ink: rasterise fills + ink and
assert no transparent pixel inside the ring's inner radius). Same picture on an
opaque white background with `drop_background: true` must give the same fill
extent (dialog route parity).

### A2 The matte's anti-alias fringe is exterior

Same file, right after `opaque` is built and `ink` is known (step 1):

- `fringe[i]` = opaque, non-ink, **reachable from an exterior pixel through
  opaque non-ink pixels within `fringe_px` of the exterior** (a bounded flood: BFS
  from exterior pixels through non-ink opaque pixels, depth ≤ `fringe_px`, default
  **1.5**, `ColourTuning::fringe_px`, range 0–4, hover: "Width (px) of the
  segmenter's soft edge outside the outline that is treated as background; 0
  keeps it as fill."), **and** with ink within `fringe_ink_px` (default 3) — the
  skirt of a line, not the edge of a line-less fill (hair top, a white collar
  touching the cut-out edge), which keeps today's behaviour.
- Fringe pixels are treated as `!opaque` everywhere downstream (set `opaque[i] =
  false` before under-painting; they then donate nothing, get no cell, and count
  as exterior in A1). For the transparent-exterior case the exterior for A2 is
  `!opaque` only (the dropped background region is not known yet; the dialog route
  has no segmenter fringe).
- Guard: never mark a pixel fringe if that leaves an ink component with no opaque
  non-ink neighbour on either side (a 1 px stroke on a cut-out edge would lose its
  donor); skip the whole fringe pass for pictures with no transparent pixel.

Test: the A1 disc with a 1 px light-grey ring added outside the dark ring
(luma 190, alpha 255) on transparent; assert the grey ring is not painted by any
fill, and that the fill colour under the ink is still red (donor unaffected).

### A3 The slab under the shoe

Diagnose with the A4 per-part renders before touching anything: render each rig
part alone on magenta at frame 0 (`rig_vs_source --parts`), run `spill.py` on each,
and name the part whose render holds the slab. Then compare that part's rendered
bounds with its trace-space bounds (`BUZZ_TRACE_SHAPES_LOG`): the offender is a
shape drawn displaced (registration / transform at `assemble_character`, or a cap
bound to the wrong joint, or a stroke width) — fix the cause, not the symptom. Record
the cause in this table. Check Guddu's 145 px blob the same way.

### A4 Diagnostics that stay

- `rig_vs_source`: `--parts` writes `<out>/<name>_parts/<slot>.png` (each rig part
  alone, every other hidden, same framing; `render_symbol --parts` is the model),
  and the JSON line gains `spill_px`, `spill_beyond_1px`, `spill_max_px` computed
  exactly as `spill.py` (figure from the source over white; render on magenta
  internally for the metric even when `BUZZ_EXAMPLE_BG` is unset — render twice or
  use alpha).
- Document `BUZZ_EXAMPLE_BG` in both examples' module docs (done for the stage
  colour; add the metric note).

### A5 Gate (Fable)

On Raju, Guddu, Woman1 front views (and one back view): `spill_beyond_1px == 0`,
`spill_max_px <= 1.0`, `mean_de` and `ink_coverage_pct` not worse than today
(Raju 2.25 / 97.9, Guddu 10.15 was measured on magenta — re-measure on white),
no new gaps between fill and ink (count transparent pixels inside the figure:
must not rise). 4× sheets on magenta: arm top, shoulder, hand, hair line, feet.
Suites: `cargo test --release -p buzz-scene`, `-p buzz-ai`, `-p buzz-ui` (lib).
Then relink `buzzanimate.exe` (the user must close the app first). Cast/episode
rebuild is the user's call (bold value still to be picked; a rebuild is ~25 min).

## Stage B — outpainting a part that is cut by its neighbour

What exists: `complete_parts_impl` (`crates/buzz-ai/src/character.rs` ~1360–1600)
extends each bone part's alpha over a **capsule** of radius 0.75 × local half-width
(capped at the bone length) plus a disc at the parent joint, only on pixels that
are hidden (owner paints over this part) and inside the matte, and **flat-fills**
the added pixels with the part's dominant colour (LaMa optional, off, never used
by the product). `trace_character_whole_with` T3 (~4406) then traces each part's
hidden pixels within `reach` (8–40 px of owned pixels, the ghost-sleeve guard) as
one `Outline` flat fill behind the part. Result: a limb that turns away from its
neighbour shows a flat disc, no outline, no continuation of its stripes or
shading, cut by a Chebyshev box — the "not complete" the user describes.

### B1 `outpaint_limb` — extrude the cut edge along the bone

New `pub fn outpaint_limb(...)` in a new `crates/buzz-ai/src/outpaint.rs`, pure
raster, deterministic, no model:

Inputs: the part's visible RGBA (`part.rgba` before completion), `allowed: &[bool]`
(pixels this part may paint: hidden under an owner that paints over it, inside the
matte — exactly today's `hole` rule), bone `head` (parent joint) and `tail`, the
source image.

1. Bone frame: unit axis `u = normalize(tail - head)`, normal `n`. Every pixel has
   `(t, s)` = (along, across) coordinates with `t = 0` at the head.
2. Visible profile: for each station `t` (1 px steps) the visible left/right
   extents `sL(t) ≤ 0 ≤ sR(t)` (min/max `s` of visible pixels at that station;
   smoothed by a 5-station median). The **cut side** is the head side when the
   visible pixels nearest the head stop at `t_cut > 0` (otherwise, for a part cut
   at its tail — a thigh under a boot — the same with the roles swapped).
3. Tube: for `t` from `t_cut` down to `-r_cap` (the cap overshoots the joint by
   `r_cap = 0.5·(sR−sL)` at `t_cut`), width = the profile at `t_cut` extrapolated
   with the median slope of the last 8 visible stations, clamped to
   [0.7, 1.3] × the width at `t_cut`; the end is a semicircle of radius `r_cap`
   centred on the head joint. `tube ∧ allowed` is the completion mask (same
   constraints as today's `hole`; nothing outside the matte).
4. Colour: pixel `(t, s)` of the tube takes the part's visible colour at
   `(t_cut + δ, s')` with `δ = 2 px` inward (off the anti-aliased cut) and `s'` =
   `s` rescaled by the width ratio of the two stations. This carries the outline
   ink (the two silhouette lines continue straight), stripes and shading. No ink
   is drawn across the cap end.
5. Non-limb parts (Chest, Hips, Head, props: anything whose bone is not in the
   limb set Shoulder/Elbow/Wrist/Thigh/Knee/Ankle, L/R) keep today's hole and take
   the **nearest visible colour** of the part (Voronoi, as `under_paint` does)
   instead of the flat dominant colour.

`complete_parts_impl` calls it for limb parts; `CompletedPart` gains
`extruded: Vec<bool>` (the tube pixels). LaMa stays as the optional textured-part
pass (`complete_parts_high_quality`), unchanged.

### B2 Trace the completion as drawn artwork

In `trace_character_whole_with` T3: the per-part hidden picture = `part.rgba` on the
hidden pixels, **plus a 2 px overlap into the part's own visible pixels** (so the
seam has no crack), traced with `TraceMode::InkAndColour` and the character's
tuning (not `Outline` + one colour), spliced at the front of `per_part_shapes[id]`
and its ink at the front of `per_part_ink[id]`. The `reach` gate stays for
non-tube hidden pixels (ghost sleeves) and is **skipped for `extruded` pixels**.
Face parts stay excluded. Joint caps (item 4) stay; they sit behind.

### B3 Editor command: Extend part

For the user's manual flow (Trace dialog on the whole figure → select the arm's
shapes → Convert to Symbol → rig or not → extend): `Command::ExtendPart` in
`crates/buzz-app` (Modify ▸ Rig ▸ Extend Part Under Neighbours…; also the agent verb
`extend_part`, `docs/AI_AGENT_PLAN.md` style). Input: the selected part (a rig part
inside an opened character, or a selected symbol instance). Axis: the part's bone
head→tail when bound; otherwise the instance's transformation point → the centroid
of the cut edge (the part's boundary pixels that touch another selected or
overlapping object). Occluders: every object above it that overlaps; `allowed` =
their union. Rasterise part and occluders at 2× with the existing symbol renderer
(`render_symbol`'s path), run `outpaint_limb`, trace with `InkAndColour`, insert the
shapes at the back of the part symbol's "Artwork" layer (ink at the back of "Ink"),
as one undo step. Headless test: a striped rectangle "arm" under a square "torso"
→ after the command, stripes continue under the torso to the pivot with a round
end, nothing outside the torso's footprint.

### B4 Gate (Fable)

Posed previews (`series_setup`'s posed render, arms down) for Raju / Guddu / Woman1:
at the shoulder, hip and ankle the uncovered strip shows the limb's own outline
continuing and its colour/stripes, no flat disc, no gap (4× crops). `rig_vs_source`
rest-pose numbers unchanged (the completion is behind). Unit test on a synthetic
striped limb (buzz-ai). Spill gate from A5 still holds (the tube is always under
an occluder at rest).

## Stage C — drawing on a traced part

The pieces exist: a character opens in place on double-click
(`enter_or_leave_at`, test `double_clicking_a_character_goes_inside_and_its_parts_can_be_selected`),
parts are symbols with "Artwork" and "Ink" layers, and brushes draw into the open
symbol. To make it dependable for this use:

- **C1** headless test (buzz-app `editor.rs` tests): open a traced character, open
  its upper-arm part in place, draw a brush stroke across the cut edge into the
  area under the torso, leave; the stroke is in the part symbol's Artwork layer,
  moves with the bone when the pose changes, and is covered by the torso at rest.
  Preview == commit (existing rule).
- **C2** while a symbol is open in place, every object outside it is already drawn
  dimmed (Animate-style); confirm that holds for rig parts (posed) and add a toolbar
  toggle "Show neighbours" (default on, 35 % opacity) so the user can see the torso
  edge they are extending under. If dimming is missing for posed parts, add it.
- **C3** Fable review: run the flow headlessly and in the real app once (open the
  app only after the exe relink).


## Round 2026-10-14 (user: "trace a character from bitmap to vector … cut the arms, hands etc. from the character, or the AI does it, then outpaint the cut sections")

Two routes, both must end with parts whose cut sections are complete:

1. **AI cuts** — AI ▸ Character from Image. Already segments into the 17 `biped_full`
   parts, completes cut sections (B1 tube, B1b borrow, B1c straight cuts, B2 traced
   completion) and builds a layer rig (LR2). W1 lets it start from the picture
   selected on stage instead of a file path. Open items there: B1d, B4b.
2. **The user cuts** — Trace Bitmap (recommended settings) → Lasso around the arm
   (the Lasso already cuts shapes along its path, `cut_and_collect`) → Convert to
   Symbol (F8) → put its transformation point on the joint → **Extend Part** (X1).

### X1 — Extend Part (replaces the B3 spec above)

`Command::ExtendPart`, label "Extend Part Under Neighbours", agent verb
`extend_part`, Modify menu under a "Rig" label after Bitmap. Works on every selected
**symbol instance** in the current timeline (stage or a symbol open in place,
layer-rig parts included). Not a symbol → status "Convert the part to a symbol first
(F8)". A part of a bone (armature) rig → status "Extend Part works on layer rigs;
convert the rig to layers in the Rig panel".

For each selected instance P at the current frame:

1. **Joint** = P's transformation point (`Scene::pivot_of`) in the timeline's space.
2. **Occluders** = every visible object drawn above P (later layers in paint order,
   and later objects on P's own layer) whose bounds overlap P's bounds grown by 25 %.
3. **Raster**: one region = bounds of P ∪ occluders ∪ a disc of radius
   0.5·max(P width, height) around the joint, clipped to that union's bounds +
   10 % margin; scale so the long side is ≤ 2048 px and ≥ 2 px per document unit
   when that fits. Render P alone (straight RGBA, transparent background) and the
   occluders alone (alpha only) into that same grid with the existing exporter
   (`buzz_export::Exporter`, a disposable scene with only those objects, as
   `render_symbol_alone` does). Verify straight vs premultiplied and un-premultiply
   if needed.
4. `visible` = P's render; `allowed` = occluder alpha > 0.5 ∧ P alpha ≤ 0.5;
   `matte` = P alpha > 0.5 ∨ occluder alpha > 0.5. **Axis**: head = joint pixel;
   tail = centroid of P's opaque pixels whose distance to the joint is ≤ the median
   such distance. Run `buzz_ai::outpaint::outpaint_limb(visible, allowed, matte,
   head, tail, &visible_image)`.
5. Nothing extruded → status says why, by case: no occluder above P ("Nothing above
   <name> hides its cut — put the part under its neighbour and run again"); joint
   inside P's visible artwork or not set ("Put the part's transformation point on its
   joint, under the neighbour, and run again").
6. **Trace** the picture = extruded pixels + P's own opaque pixels within 2 px
   (Chebyshev) of them (seam overlap), with `TraceOptions::ink_colour()
   .with_saved_tuning()`, `drop_background: true`. Map the shapes pixel → timeline
   space → P's own space (inverse of P's transform) → symbol space.
7. **Insert** as one undo step "Extend Part": a new layer **"Extension"** at the
   **bottom** of P's symbol (behind all its artwork) holding the traced shapes on
   the frames P's symbol shows at the current frame (a keyframe at the symbol
   frame shown, extended over the symbol's span). Running it again replaces that
   layer's contents (no stacking). Every instance of that symbol shares it.
8. Status: "Extended <n> part(s) under their neighbours" / per-case reasons.

Proof (no new tests, per the user 2026-10-11): an example
`crates/buzz-app/examples/extend_part_check.rs` that builds a scene — a striped
rectangle arm symbol (outline + 3 stripes) under a square torso shape, joint inside
the torso — runs the command headlessly, renders before/after with the torso hidden
and with the arm rotated 60° about the joint, and writes PNGs to a given folder.
Pass: stripes and outline continue under the torso to the joint with a round end,
nothing painted outside the torso's footprint at rest (diff of rest renders with the
torso visible = 0 px beyond anti-alias), undo removes the layer.

### X2 — B1d (spec above, unchanged)

### Gate (planner): X1 sheets looked at; B4b on posed Raju/Guddu/Woman1; spill gate
from A5 holds; suites green (buzz-ai, buzz-ui `--test-threads=1`, buzz-app lib).
