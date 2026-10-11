# Bone constraints in the Bone tool + a Layer-parenting rig option

User, 2026-10-07: "the rigging is just bone tool for now, I want an option for
layer parenting rig. In the bone tool rig there is no option of angle
constraints — bones shall not rotate beyond a certain degree and direction
shall also be there. These options shall be UI configurations as well."

Runs after `docs/TRACE_TUNING_PLAN.md` (one implementer at a time).

## Status

| Step | State | Date | Evidence |
|---|---|---|---|
| Findings (Fable) | done | 2026-10-07 | below |
| R1 limits relative to rest, direction + lock, every angle writer clamps, pattern defaults | **done — Fable-reviewed** (buzz-rig 100, buzz-act 173, buzz-scene 529, buzz-ai 182, buzz-app rigging_panel 9 / bone_rig 4 / pose_library 5 all green on re-run) | 2026-10-07 | R1.1 buzz-rig done: `JointLimits::{from_relative, relative_to, direction}`, `Direction`, `RotationMode`, `Bone::rotation_mode`, `JointLimitDefaults::anatomical` + `RigPattern::apply_limits` (biped/biped_full filled); tests `relative_limits_round_trip`, `locked_is_min_equals_max`, `elbows_and_knees_bend_one_way_on_both_sides`; buzz-rig 99/99. R1.2 buzz-act done: `autorig::assemble`/`assemble_from_joints` store `JointLimits::from_relative(bone.rest_angle, slot)`; new `tests/joint_limits_and_the_director.rs` (3): `an_auto_rigged_bone_is_limited_about_its_rest_angle`, `the_directors_verbs_are_never_clamped` (all 10 verbs, 241 phases, amount 1.0 — first run flagged React clamping Elbow L 4.84° / Elbow R 4.29°, so elbows widened −150..0 → −150..+10), `a_verb_cannot_bend_a_clockwise_only_elbow_the_other_way` (Talk on a CW-only Elbow R holds 0°, default bends); `an_auto_rigged_character_walks` unchanged and green; buzz-act 173/173. R1.3 clamp audit: no direct `Bone.angle` writer outside buzz-rig except tests; every writer already goes through `Armature::set_pose` (clamps): buzz-act `perform.rs` 806, `puppet.rs` 482, `physics.rs` 168; buzz-scene `modifier.rs` 742, `rig.rs` 442 (`tween_armature`); no `set_angle` added. New buzz-scene test `a_tween_stays_inside_the_joint_limits` (legal↔legal stays in range; legal→illegal literal 2.0 tweens to 0.5 at t=0.5 and 1.0); buzz-scene 529/529. R1.4 buzz-ai run: first `motion_and_poses` failed 2: `standard_poses_bend_a_traced_characters_own_rest_pose` (walk, Shoulder L stored −3.272 for 3.011 — same direction, re-expressed a turn round by `JointLimits::clamp`) and `wave_raises_the_hand_above_the_elbow_on_a_bent_rest_arm` (wave asks Elbow R +107°, clamped at +10°). Fixed: `Bone::constrain` keeps an angle the limits do not stop exactly as written (test `an_angle_inside_the_limits_is_kept_as_written`); Elbow R default widened to +150. buzz-rig 100/100, buzz-ai 182/182 (9 ignored, pre-existing); no test edited. |
| R2 joint widget in the Bone tool's Tool Options + Rig panel, stage arc, apply-to-chain/all | **done (awaiting review)** (R2.1–R2.5; R2.5 3 of 3 tests) | 2026-10-07 | **R2.3 done (decision taken):** `Editor.selected_bone: Option<(ObjectId, usize)>`, set in `begin_rig_gesture` on a Bone-tool press on a bone body or tip (pose and build-from-tip); `Editor::forget_bone_if_deselected()` (called at the top of `App::build_ui`) clears it when the selection is not exactly that rig. `rigging::selected_bone(scene, selected: Option<(ObjectId, usize)>)` now reads it (was `bones.first()`). Tool Options (`draw_panel_inner` ToolOptions) → `App::bone_tool_joint_options`: Bone tool + one armature selected + bone → "Joint limits — {name}" + `joint_widget::joint_limits`; armature but no bone → "Click a bone to set its limits" + mode/direction/range row + "Apply to all bones" (`JointEdit::ApplyToAll`); edits via `App::apply_joint_edit` (one undo step). Test `tool_options_shows_the_joint_widget_for_a_selected_bone` (headless: Bone tool, `pointer_down/up` on bone 1 via camera, painted-text check of a full `build_ui` frame: hint before, "Rotation" + "Joint limits — Bone 2" after, Locked edit undoes in one step, clearing the selection forgets the bone) green; `joint_limit_tests` 4/4, rigging_panel 9 / bone_rig 4 / pose_library 5 green. Previously **R2.3 blocked:** there is no selected-bone state anywhere — `rigging::selected_bone(scene, object)` returns `bones.first()` of the selected armature (nothing calls it), `RigResponse.select_bone` is never set or read, and the Bone tool's `RigGesture::Posing { bone }` is not kept after the drag. Wiring the Tool Options widget to it would always edit bone 0 (Hips, Free), and "armature selected but no bone" never happens. Needs a decision: add `Editor.selected_bone: Option<(ObjectId, usize)>` (set on Bone-tool press on a bone/tip, cleared when the selection changes), and make `selected_bone` read it? Test `tool_options_shows_the_joint_widget_for_a_selected_bone` waits on that. R2.4 done: `rigging::stage_joint_arcs(scene, frame)` (edges = parent's posed world angle + stored limit, stage space via the object transform, mirrored rigs sweep from `max`), `stage.rs` `draw_rigs` draws a wedge per limited joint under the bones (radius `bone_width × 2.5`, selection colour × 0.4 when the rig is selected else bone colour α 102, quarter-turn convex slices + outline), Locked = filled dot r 4; only when bones are shown. R2.5: headless tests in `app.rs` `joint_limit_tests` (in-crate because `App::build_ui`/`editor` are crate-private): `locking_a_joint_stores_min_equals_max_at_rest`, `apply_to_chain_reaches_descendants_only` (each also undoes in one step and draws a full `build_ui` frame) — green. Earlier: R2.1 done: `buzz-ui/src/joint_widget.rs` — `joint_limits(ui, bone, index) -> Option<JointEdit>`, row pieces `mode_buttons` / `direction_buttons` / `range_values` on a `LimitSpec = (RotationMode, Direction, min°, max°)` relative to rest, `spec_of`, `relative_limits`, `limits_about`. `JointEdit::ApplyToAll.limits` is **relative to rest** (no one bone to be absolute about); `Set`/`ApplyToChain` absolute about bone `index`. R2.2 done: Rig panel bone row uses the widget (Limit checkbox + bare drag values + "{angle}°" label removed); `RigResponse.apply_limits: Option<JointEdit>`; app.rs `App::apply_joint_edit` → `rigging::apply_joint_edit` (chain = `Armature::subtree`, re-expressed about each bone's rest), one `doc.edit` each. buzz-ui 370 lib + all integration green except the 3 known `dock_columns` failures (Layer Depth / layer rows, not Rig); buzz-app rigging_panel 9 / bone_rig 4 / pose_library 5 green. |
| R3 `rig_limits.json` defaults (per slot) editable in the UI, used by every rig builder | done (awaiting review) | 2026-10-07 | R3.1: `crates/buzz-app/src/rig_limits.rs` — `path()` (`BUZZ_RIG_LIMITS` or `%APPDATA%/BuzzAnimate/rig_limits.json`, same base as presets), `load_or_default()` (file rows laid over `anatomical()`), `save(&table)`, `pattern_with_saved_limits(name)` (unknown name → biped). Callers: `App::rig_character` (Rig panel Build); buzz-ai `assemble_character(.., limits: Option<&JointLimitDefaults>)` and `rerig_vector_character(.., limits)` (None = unchanged); buzz-ui `AiCharacterState.rig_limits` (filled lazily by the app before the dialog / AI panel draw, refreshed on Save as default) → `create_character`; `character_from_image(_with)(.., limits, ..)` ← agent task loads the file; app's vector re-rig loads the file; `series_setup` passes `load_or_default()`. `episode_build` builds no rigs (uses series_setup's Cast assets) — nothing to wire. Call sites given `None`: buzz-ai tests assemble_character ×2, biped_full ×2, face_rig ×4, rig_quality ×2, quality_redesign ×1, motion_and_poses ×1, rerig_vector_character ×2; app.rs tests ×2; examples ai_character, ai_real_check, rig_vs_source. Side effect: Quadruped/Bird `Head` slot now gets the table's `Head` row (±40°) through the Rig panel Build, since the table is keyed by slot name. R3.2: Rig panel collapsing **Default limits** (closed) under the slot list — one row per slot of the selected rig's pattern (else the assembly pattern) using `joint_widget::{mode_buttons, direction_buttons, range_values}`; **Save as default** (→ `RigResponse.save_default_limits`, app saves + status), **Reset to anatomical** (working table only; kept on Save), **Apply to this rig** (→ `rigging::apply_default_limits`, one undo step, bones the table does not name untouched). `RigPanelState.default_limits` loaded from the file on first draw. `buzz_ui::rig_panel::default_limit_rows(pattern, table)`. Test `default_limits_table_lists_every_slot_of_the_pattern` green. buzz-ai 182/182 (9 ignored) after the signature change. |
| L1 `rig_character_layers`: parts → one layer each, pivot at the joint, `follows` chain; headless test | done (awaiting review) | 2026-10-07 | buzz-act `autorig::fit_slots(pattern, parts)` (new pub fn, the same `fit` `assemble` uses; `assemble` untouched). buzz-app `rigging.rs`: `LayerRig { layers: Vec<(usize, LayerId)>, root }`, `rig_character_layers(scene, frame, pattern, slots)` (paint order as `rig_character`; parts leave their layers; one new layer per part named after the slot, in front of the frontmost part's layer, same folder, lengthened to its length; `object.pivot` = fitted head in object space, bounds centre if unmeasurable; `set_follows(child, nearest filled ancestor's layer, frame)`). **Choice beyond the Design, flag for review:** a part with no filled ancestor other than `root` follows `root` (biped thighs are root slots, so legs would otherwise not follow the hips). Conversions beside it: `bones_to_layers(scene, frame, rig)` (rigid parts at their posed stage transform, pivot = posed bone head, parent = nearest ancestor bone with a part; **refuses a rig with skinned parts**, nothing changed), `layers_to_bones(scene, frame, pattern, layers)` (slot from layer name, inherited transform baked into each part, links + rest poses cleared, `rig_character`, emptied rig layers removed). Not yet wired to undoable app actions (L2). New `crates/buzz-app/tests/layer_rig.rs` 2/2: `turning_the_upper_arm_carries_the_forearm_about_the_shoulder` (chest/upper arm/forearm bars → 3 layers, follows chest←upper←fore, pivots at (200,10)/(140,10) within 0.5 px; upper arm turned 30° about its pivot on frame 10 → `inherited_transform(fore, 10)` maps the elbow to the turned elbow within 0.01 px; chest transform unchanged, its motion identity), `bones_to_layers_and_back_keeps_the_skeleton` (names equal, lengths < 0.5 px, rest angles < 0.5°, one rig layer left holding the armature). buzz-rig 100/100, buzz-scene 529/529, buzz-app rigging_panel 9 / bone_rig 4 / pose_library 5 / joint_limit_tests 4 green. |
| L2 "Rig kind: Bones / Layers" in the Rig panel, the AI character dialog, `series_setup --rig`; Convert buttons both ways | done (awaiting review) | 2026-10-07 | `buzz_rig::RigKind { Bones (default), Layers }` (serde lowercase, `label`/`parse`, `LAYERS_HINT` = the Design line). (a) buzz-ui `rig_panel::rig_kind_row` (radio + hint shown when Layers) above **Rig Character**; `RigPanelState.{kind, layer_rig}`; `RigResponse.{convert_to_layers, convert_to_bones}`; **Convert to layer rig** when an armature is selected, **Convert to bone rig** when the selection's layer is in a follow tree (`App::selected_layer_rig` → `rigging::layer_rig_of`). App: `rig_character` builds `rig_character_layers` when kind = Layers (one `doc.edit` "Rig Character (Layers)", parts selected); `convert_to_layer_rig` = one `doc.edit` around `bones_to_layers` (refusal → status "Cannot convert: … skinned … Nothing changed.", no undo step since the scene is untouched); `convert_to_bone_rig` = `rigging::pattern_for_layers` (most slot-name hits, ties → fewer slots) + saved limits + one `doc.edit` around `layers_to_bones`. (b) `AiCharacterState.rig_kind` + the same radio in the Preview step; `AiCharacterDialogAction::Created(Box<AssembledCharacter>, RigKind)`; app handlers call `Editor::create_ai_character_as(&a, kind)` (`create_ai_character` = Bones); Layers clones the character and runs `rigging::convert_all_to_layers` on its scene before the Cast save and merge (skinned → stays bones, status says so); placing it now also shifts merged layers' `rest_pose` by the centring move (no-op for bone rigs: buzz-ai makes no follows). (c) `series_setup --rig bones\|layers` (default bones): after `bake_pose`, `convert_all_to_layers`; refusal is an error; report gets `"rig"`. Compiled (`cargo check -p buzz-app --example series_setup --lib`), not run. (d) Free Transform Tool Options: "Rotating this part carries the parts that follow it" when the selected object's layer has followers. Tests (app.rs, headless `App::new` + `build_ui` painted text): `layer_rig_ui_tests::rig_kind_radio_changes_what_build_produces` (Bones → 1 armature, no links; Layers → "Rig kind:" + hint painted, 0 armatures, Chest←Shoulder L←Elbow L, one undo step, undo clears links), `converting_bones_to_layers_and_back_is_one_undo_step_each` (buttons painted; +1 undo each; Free Transform hint painted on the chest; names/lengths <0.5 px/rest <0.5° kept; undo → same layer links, undo → the original armature id and bones equal), `a_skinned_rig_refuses_to_convert` (no undo step, status mentions skinned); `idle_tests::an_ai_character_created_as_layers_is_a_layer_rig` (pujariFront: 0 armatures, followed layers, one undo step; the Cast asset loads with 0 armatures and the same follower count). Green: those 4, joint_limit_tests 4, rigging_panel 9, bone_rig 4, pose_library 5, layer_rig 2, buzz-rig 100. buzz-ui lib 370/370 with `--test-threads=1`; in parallel `timeline_panel::shift_click_sweeps_a_range_and_ctrl_click_toggles_one` fails (0 rows; passes alone 3/3; untouched file). |
| L3 Cast assets carry layer rigs; Cast drop places one | done (awaiting review) | 2026-10-07 | Done: a layer rig in the Cast is the scene with parts on followed layers and no armature (AI dialog Layers / `series_setup --rig layers` save exactly that). Cast drop: `Editor::place_asset_at` and `create_ai_character_as` now call `carry_rest_poses` (shift each arrived followed-against layer's `rest_pose` by the placement move); without it a dropped layer rig's followers inherited the move (Elbow L got translate(860, 680) — test failed with the call removed, passes with it). The Cast panel's **Cast** button (merge in place, no move) needed nothing. Test `tests/layer_rig.rs::a_layer_rig_cast_asset_round_trips_and_drops_with_follows_intact` (save → load keeps both links + both pivots, 0 armatures; `place_asset_at` at (600,400): one undo step, links equal, every follower's `inherited_transform` identity) green; layer_rig 3/3. **Rig kind (decision (a) taken):** `buzz_doc::Asset.rig_kind: Option<RigKind>` from `buzz_doc::rig_kind_of(scene)` (armature on any main or library-symbol layer → Bones; else any layer with `follows` → Layers; else None), read at scan in the same mtime-cached load as `animated` (cache entry now `(mtime, animated, rig_kind)`); no file-format change, no sidecar. Cast panel row (`character_panel::character_row`) shows a small weak "bones"/"layers" label beside **Cast**. Tests green: buzz-doc `assets::tests::scanning_a_cast_folder_reports_each_rig_kind` (Boned/Layered/Oak → Bones/Layers/None; buzz-doc lib 231/231); buzz-app `layer_rig_ui_tests::the_cast_panel_row_shows_each_characters_rig_kind` (rig_character Bones + rig_character_layers saved to Cast, rescan reports both kinds, full `build_ui` frame with Cast revealed paints "bones" and "layers"; module 4/4); layer_rig 3/3, cast_wardrobe 3/3, buzz-ui character_panel 2/2. |
| Fable review | pending | | |

## Findings (2026-10-07)

**Joint limits already exist, but you cannot find them and the rigs never get
them.**

- Data: `buzz_rig::Bone.limits: Option<JointLimits { min, max }>`
  (`crates/buzz-rig/src/lib.rs` 43–141). The IK solver clamps every pass
  (`ik.rs` ~146, tests `…outside its limits`, `limits_that_make_a_target_impossible…`).
  `Bone::constrain` clamps `set_angle`.
- UI: only the **Rig panel** (`crates/buzz-ui/src/rig_panel.rs` 917–961): a
  "Limit" checkbox and two bare drag values, nothing in the Bone tool's Tool
  Options, nothing drawn on stage. That is why it reads as "no option".
- **Bug:** limits are absolute parent-relative angles, and the checkbox starts
  a joint at **−90°..+90° about zero**, not about the rest angle
  (`FRAC_QUARTER`, rig_panel.rs 929). A limb drawn at 100° from its parent is
  outside its own fresh limit; the next IK pass snaps it.
- Every auto-rig pattern slot has `limits: None` (`buzz-rig/src/pattern.rs`
  102; `biped()` 122, `biped_full()` 248): every AI / Cast rig has free joints,
  so elbows and knees bend backwards.
- Writers of `Bone.angle` outside buzz-rig (director verbs, tweens, poses,
  follow-through) may bypass `constrain` — audit in R1.

**Layer parenting already exists, but nothing builds a rig with it.**

- Data: `Layer::follows`, `follows_bone`, `rest_pose`,
  `Layers::inherited_transform` (`buzz-scene/src/layer.rs` 148–190, 613):
  a follower inherits the followed layer's **first object's** motion away from
  its rest pose, so rotation about that object's `pivot` (`object.rs` 512)
  carries the children — exactly Animate's layer parenting.
- UI: timeline parenting view (node graph, drag to link; `timeline_panel.rs`
  933) and the layer menu "Parent To" / detach (3159). `Scene::set_follows`
  records the rest pose.
- Missing: every rig builder (`rigging::rig_character` 470, the Rig panel's
  Build, the AI character dialog's `create_character`, `series_setup`,
  Cast assets) produces an `ObjectKind::Armature` only.

## Design decisions

- **Limits are edited relative to the rest angle** (min ≤ 0 ≤ max, degrees);
  stored as today's absolute `JointLimits` (`rest_angle + min`, `rest_angle +
  max`) so saved documents and the IK code do not change. A new limit starts
  at rest ± 90°.
- **Rotation mode** per bone: `Free` (no limits) · `Limited` (min/max) ·
  `Locked` (min = max = 0 → the bone holds its rest angle; Animate's "Enable
  rotation" off).
- **Direction** per bone, only in `Limited`: `Both` · `Clockwise only`
  (min = 0, max = N) · `Counter-clockwise only` (min = −N, max = 0). Stored as
  the same two numbers; the widget reads the direction back from their signs.
  Screen-space clockwise (y down) — label the buttons with curved arrows so
  nobody has to remember which sign is which.
- **Pattern defaults** (R1/R3) — `JointLimitDefaults::anatomical()`
  (`buzz-rig/src/pattern.rs`), relative to rest, degrees, **the same on both
  sides, no L/R mirroring**. Revised 2026-10-07 (decision (a)): the director's
  verbs (`buzz-act/src/perform.rs` `pose_at`) bend *both* elbows and *both*
  knees negative, so a mirrored limit would freeze one elbow and one knee in
  every walk/run/talk/sit/react/reach on every auto-rigged and Cast rig. The
  director is the ground truth; a limit that clamps a verb is wrong by
  definition (widen the slot, never narrow the verb):
  Hips (root, carries the whole lean) Free · Chest ±30 · Head ±40 (no
  neck/spine slot, so these carry them) · Shoulder L/R ±170 · Elbow L −150..+10
  · Elbow R −150..+150 · Thigh L/R (the plan's "hip") −120..+90 (covers sit's
  +77°) · Knee L/R −150..0 (CCW only) · Wrist ±80 · Ankle ±45 · Brow ±15.
  Widened during R1 by `the_directors_verbs_are_never_clamped` and the buzz-ai
  suite: elbows 0 → +10 (React overshoots 4.84° L / 4.29° R past rest);
  Elbow R → +150 (buzz-ai `PoseLibrary` "wave" resolves the right elbow to
  +117° on the plain biped, +107° on the traced pujariFront character — it
  bends the other way from the director's verbs, so the right elbow is
  two-way). No slot was flipped. The user can change any slot later (R3 UI).
- **Layer rig** (`RigKind::Layers`): one layer per filled slot, named after the
  slot, ordered like the armature's draw order; the part object alone on it;
  `object.pivot = Some(joint head)` (the fitted bone's head, `autorig`'s
  `fitted[index]`); `scene.set_follows(child, nearest filled ancestor's layer,
  frame)`. Posing = Free Transform rotation about the pivot (existing tool).
  No bones, so no IK, no director verbs, no lip-sync: the kind picker says so
  in one line ("Layer rig: pose with Free Transform; the director and
  lip-sync need a bone rig").
  *2026-10-11 (PRODUCTION_SPEED_PLAN LR1/LR2):* the director, gestures, blinks
  and lip sync now perform layer rigs (`buzz_act::layer_rig::rig_of`), and
  Layers is the default kind for new characters (Rig panel, AI character
  dialog, `series_setup --rig`, the Assistant's `ai_character_from_image`
  `rig`); `RigKind`'s own `#[default]` stays Bones. The hint now reads "Layer
  rig: pose with Free Transform; the director, gestures, blinks and lip sync
  work on it; joint limits are bone-rig only".
- **Convert** both ways (Rig panel): Bones → Layers (`ArmatureData.parts` +
  their bone binding → layers as above, pivot at the bone head, pose kept as
  the rest pose); Layers → Bones (`rig_character` with the slots read back
  from the layer names). Converting is an undoable edit.

## R1 — buzz-rig + buzz-act (data, defaults, clamp audit)

- `JointLimits`: add `pub fn relative_to(rest) -> (f64, f64)` and
  `from_relative(rest, min, max)`; `pub fn direction(&self, rest) -> Direction
  { Both, Clockwise, CounterClockwise }`; `Bone::rotation_mode()`.
- `pub enum RotationMode { Free, Limited, Locked }` — derived from `limits`,
  not stored (`Locked` = min == max within 1e-9).
- `pattern.rs`: `Slot.limits` filled from `JointLimitDefaults::anatomical()`
  (a `name → (mode, direction, min_deg, max_deg)` table; serde; the R3 file
  overrides it). `biped()`/`biped_full()` tests updated: a test that an elbow
  slot is CW-only 0..150 and its mirror is CCW-only.
- Audit: `grep -rn "\.angle = \|\.angle +=" crates --include=*.rs` outside
  buzz-rig (buzz-act perform/direct/puppet, tween_armature, poses, follow.rs).
  Route each through `Bone::set_angle` (public, clamps) or clamp explicitly;
  `tween_armature` must clamp the interpolated angle. Test: a director verb
  that asks an elbow for −60° on a CW-only joint lands at 0°.
- `buzz-rig` tests green; `buzz-act` tests green (`director_verbs`,
  `one_shot_actions`, `follow_through_bake`, `wiggle_bake`,
  `rig_a_drawn_character` — rest-pose rigs stay inside their new limits by
  construction, so expect no posed-frame change unless a verb over-bent; list
  any that did in the Status table).

## R2 — UI: joint widget, Tool Options, stage arc

- `buzz-ui/src/joint_widget.rs`: `pub fn joint_limits(ui, bone: &Bone) ->
  Option<JointEdit>` — row 1: Rotation `Free | Limited | Locked` (selectable
  values); row 2 (Limited): Direction `⟲ Both | ↻ Clockwise | ↺ Counter`;
  row 3: Min / Max drag values in degrees relative to rest with the range
  clamped by the direction; hover texts in plain words; a small weak label
  "now +23° of rest". Buttons: **Apply to chain** (this bone and every
  descendant gets the same mode/direction/range) · **Apply to all bones**.
- Rig panel: replace lines 917–961 with the widget.
- Tool Options (`PanelId::ToolOptions`, app.rs 4288): when the active tool is
  Bone and a bone is selected (`rigging::selected_bone`), show the widget;
  when an armature is selected but no bone, show "Click a bone to set its
  limits" + the Apply-to-all row; otherwise the Bone tool's existing options
  (if none, just the hint).
- Stage (`stage.rs` 1180–1199): for a bone with limits, draw the allowed
  wedge at the joint: a translucent sector from `rest + min` to `rest + max`
  (world angles: parent world angle + rest_angle + offset), radius = the
  bone-width scale × 2.5, selection colour when selected, else the bone
  colour at 40 %; `Locked` draws a small padlock-sized filled dot. Hidden when
  the rig overlay is hidden.
- Headless UI tests (App::new + build_ui, no real clicks): the Tool Options
  panel shows the widget for a selected bone; changing mode to Locked stores
  `min == max == rest`; Apply-to-chain reaches the descendants only.

## R3 — defaults file + UI

- `rig_limits.json` in the app data dir (same base as `presets.rs` 118;
  `BUZZ_RIG_LIMITS` env overrides the path): the `JointLimitDefaults` table.
  `load_or_default()`, `save()`.
- Rig panel, collapsing **Default limits** (closed): one row per slot of the
  current pattern (mode, direction, min, max), **Save as default**, **Reset to
  anatomical**, **Apply to this rig**. Every rig builder (Rig panel Build, AI
  character dialog, `series_setup`, `episode_build`) reads the file through one
  function `pattern_with_saved_limits(name)`.

## L1 — layer rig builder (buzz-app `rigging.rs` + buzz-scene)

- `pub fn rig_character_layers(scene, frame, pattern, slots: &[Option<ObjectId>]) -> Option<LayerRig>`
  as in Design; `LayerRig { layers: Vec<(slot, LayerId)>, root: LayerId }`.
  Reuse `autorig`'s fitting for the joint heads (`fit_slots` or whatever
  `rig_character` calls — read `rigging.rs` 470–532 and `autorig.rs` 60–120).
- Headless test in `crates/buzz-app/tests/`: three rectangles (chest, upper
  arm, forearm) → layer rig; rotate the upper-arm object 30° about its pivot
  on frame 10 → `inherited_transform(forearm layer, 10)` maps the elbow point
  to the rotated elbow within 0.01 px; chest unmoved.
- Bones ↔ Layers conversion functions beside it, with a round-trip test
  (bones → layers → bones keeps slot names, lengths within 0.5 px, rest
  angles within 0.5°).

## L2 — UI and CLI

- Rig panel Build row: `Rig kind: ◉ Bones ○ Layers` + the one-line hint;
  **Convert to layer rig** / **Convert to bone rig** when a rig is selected.
- AI character dialog (`ai_character_dialog.rs` `render_preview` ~829 / `create_character`
  895): the same radio; `AiCharacterDialogAction::Created` carries the kind;
  app.rs's handler (grep `AiCharacterDialogAction::Created`) builds layers
  when asked.
- `series_setup --rig bones|layers` (default bones; default layers since
  2026-10-11, PRODUCTION_SPEED_PLAN LR2).
- Layer rigs get their own Tool Options hint under Free Transform: "Rotating
  this part carries the parts that follow it".

## L3 — Cast

- `AssembledCharacter::save_to_cast` already stores a `Scene` inside the
  character symbol; a layer rig is that scene with the parts on followed
  layers and no armature. Store `rig_kind` in the asset metadata; Cast drop
  places either kind; the Cast panel's row shows "bones" / "layers".

## Acceptance (Fable review)

- Rig panel and Bone-tool Tool Options show the widget; the stage draws the
  wedge; a CW-only elbow cannot be dragged backwards (headless `pose_bone`
  test + a real-app `BUZZ_FRAME_LOG` still).
- Raju Cast rig rebuilt with defaults: a director "wave" and "walk" do not
  bend elbows/knees backwards (compare `episode_v3` stills).
- Layer rig of Raju placed from the Cast: Free Transform on the upper arm
  carries the forearm and hand; undo works; file round-trips.
- All suites green: buzz-rig, buzz-act, buzz-scene, buzz-app, buzz-ui.
