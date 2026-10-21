# Layer management: filter, clean up, auto-arrange

User, 2026-10-20: "I don't want to see lots of empty layers from last keyframes
or stages in the timeline, they take up space." Then: "implement all. I want
auto arrange etc. to be more efficient."

"Efficient" here means two things. Arranging happens **automatically** after
the builds that make the mess, without a manual pass. And the filter costs
nothing per frame: emptiness is cached per scene revision, never recomputed
while drawing.

## What exists (measured 2026-10-20)

* Folders with disclosure triangles. `Layer::collapsed`, and
  `LayerStack::is_row_hidden` (`buzz-scene/src/layer.rs` ~590) is the single
  rule for which rows are drawn. `visible_row_order` (~614) is used by
  `timeline_panel.rs` ~250/~975/~2726, `panels.rs` ~4267/~4447 and
  `editor.rs` ~20620.
* Row height: `TimelineState::row_scale` (`timeline_panel.rs` ~433, control
  ~1189), saved in `Workspace::row_scale` (`workspace.rs` ~451). Per layer:
  `Layer::height`.
* `Layer::parent` is folder nesting. `Layer::follows`/`follows_bone` is
  layer-parenting rigs.
* There's no empty-layer handling and no arrange command.

## L1: Timeline layer filter (view only)

* `buzz_ui::timeline_panel::LayerFilter { All, HideEmpty, HereOnly }`. It's
  saved in `Workspace::layer_filter` (`#[serde(default)]`, All), and there's a
  free-text `layer_search: String` (not saved).
* **HideEmpty** hides a layer with no objects and no sound in any keyframe.
  A folder hides when every child is hidden.
* **HereOnly** shows only layers with objects on the playhead frame (sound
  layers count if their sound spans the playhead), plus every ancestor folder
  of a shown layer.
* **Search** shows only layers whose name contains the text
  (case-insensitive), plus ancestors. It combines with the filter.
* Always shown: the active layer, selected layers, and layers holding a
  selected object.
* One function `row_filter_hidden(scene, id, filter, ctx) -> bool`, combined
  with `is_row_hidden` everywhere `visible_row_order`/`is_row_hidden` are used
  for **drawing and Shift-range** (timeline + Layers panel). Not for
  rendering, export, or editor logic that addresses layers by id.
* **Cache:** a `HashMap<LayerId, (has_content, first, last)>` keyed by
  (scene revision, editing path), built once per revision. HereOnly checks the
  playhead against the span list from that cache. No per-frame walk over
  objects.
* UI: in the timeline header, a small dropdown (All / Hide empty / Only at
  playhead), a search field, and when rows are hidden a "N hidden" label
  (click → All). Plus a **Compact** toggle that sets `row_scale` to 0.6 and
  back to the previous value.

## L2: Clean Up Layers (deletes, undoable)

* **Timeline ▸ Clean Up Layers…** shows a confirm with counts, then applies in
  one undo step ("Clean Up Layers").
  * **Delete empty layers**: no objects and no sound in any keyframe, not a
    folder with children, not the target of another layer's `parent`/`follows`,
    not a Mask with Masked layers under it, not an `audio_guide` layer.
  * **Trim trailing blank keyframes**: on each layer, a run of blank keyframes
    after its last non-empty keyframe is removed and the layer shortened.
    Never shorten past the timeline's own length if that would shorten the
    film (the longest layer keeps the scene length). In practice: only trim
    layers that end past their content, down to the scene's longest
    *content* end.
  * **Remove empty folders**: folders with no children.
  * **Scope**: "This timeline" (default) or "Whole scene" (the stage plus every
    library symbol).
* Lives in buzz-scene as `LayerStack::cleanup_plan(&self) -> CleanupPlan` plus
  `apply`, so preview and apply share one rule.

## L3: Arrange Layers (auto folders, drawing order safe)

* **Timeline ▸ Arrange Layers** in one undo step. Classify each top-level
  layer, folders left as they are:
  * **Character `<name>`**: holds an instance of a character symbol (tag
    `character`, folder `Cast`/`Cast/...`), plus every layer that `follows` it
    (a layer-parenting rig chain stays together). Named after the symbol.
  * **Audio**: has sound and no objects. Pinned to the bottom.
  * **Background**: name matches `bg|background|sky|backdrop|floor|ground`
    (case-insensitive, whole word or prefix), or it holds only bitmaps that
    cover at least 60% of the stage.
  * **FX**: name matches `fx|effect|light|glow|shadow|particle`.
  * **Props**: everything else with content.
  * Empty layers go to the cleanup pass. Arrange runs L2's "delete empty
    layers" on the same timeline first, so the result has no junk.
* **Drawing order is never changed visibly.** Each group becomes a folder
  holding its layers. Moving layer A past layer B to make a group contiguous
  is allowed only if their content bounds (union over all frames) don't
  intersect. Otherwise A stays where it is, outside the folder, and the report
  counts it. Bounds come from the same memoised resolved-bounds table the
  scene already keeps.
* Folders are created collapsed, except the one holding the active layer.
  Existing folders with a matching name are reused, so re-running is
  idempotent.
* **Auto-tidy** (`Workspace::auto_tidy_layers`, default **on**): run
  Arrange (with its empty-layer cleanup) automatically and silently, with a
  status line, after: episode build / Producer scene build, Set Up Shot from
  setup / stamp, XFL/FLA import, AI scene direction. One undo step, labelled
  "Arrange Layers". Hook at each build's end in the editor (grep for where
  those finish and refresh the timeline).
* **Focus selection** (`Workspace::focus_folders`, default off): when the
  active layer changes, expand its ancestor folders and collapse the other
  top-level folders. View-only collapse (no undo step) — `collapsed` is
  document state today, so a collapse change here goes through `edit_view`
  only if the layer stack allows it. Otherwise it's a plain edit with the
  label "Focus folders", coalesced.

## Constraints

* No new tests. Existing suites green (`buzz-scene`, `buzz-ui`,
  `buzz-app --lib`; known pre-existing failures: `dock_columns` ×3,
  `every_ai_command_reveals_the_ai_panel_and_it_draws_through_the_ui`).
* Never `cargo fmt`. Don't touch the uncommitted drawing-mode or master-audio
  work beyond compiling.
* Verify headlessly: scratch probe on an episode-built document (ep1) for
  hidden counts, cleanup counts, and arrange without any z-order change (render
  frame hashes are not stable, so compare per-layer paint order of
  overlapping layers before/after).

## Status

| Step | What | State |
|---|---|---|
| L1 | Filter + search + N hidden + Compact, cached emptiness | done 2026-10-20: `LayerFilter`/`RowFilter`/`LayerRowCache`/`row_filter_hidden` in `timeline_panel.rs`; timeline rows, frame-drag rows, Shift-range and Layers panel (`layers_panel_filtered`) use it; cache on `Editor::layer_rows`, search `Editor::layer_search` (unsaved), `Workspace::layer_filter` + `row_scale_before_compact` saved. Scratch probe (deleted): 8 layers → All 8, HideEmpty 3 (5 hidden), HereOnly f0 = body+sound, f15 = head+sound; active empty layer kept; pinned kid shows its folder; "raj" × All/HideEmpty/HereOnly combine; 1 build per revision, none with All. ep1 (`scratchpad/produce/p5/ep/ep1.buzz`, 131 layers): All 131, HideEmpty 121, HereOnly@0 120; cache 0.7 ms, view 0.02 ms. `LAYER_TOOLS_INLINE_WIDTH` 860 → 1190 (header row now ~1121 pt). Suites: see below. |
| L2 | Clean Up Layers (plan/apply in buzz-scene, confirm, scope) | done 2026-10-21: `buzz-scene/src/cleanup.rs` — `LayerStack::cleanup_plan`/`cleanup_plan_with(opts, protected)`/`apply_cleanup`, `Scene::cleanup_preview`/`cleanup_layers(scope, opts)` → `CleanupReport`; shared `Layer::is_empty_layer` + `LayerStack::is_depended_on` (master_audio.rs now calls them). Extra guards beyond spec: labelled keyframes count as content, a layer whose removal would join a Masked/Guided run to a mask/guide above is kept, stage layers a light sits above are kept, a timeline never loses its last layer. UI: Modify ▸ Timeline ▸ Clean Up Layers… (`buzz-ui/src/cleanup_dialog.rs`, 3 checkboxes, scope radio, live counts cached per revision/scope/opts, one `doc.edit("Clean Up Layers")`, status line); agent `clean_up_layers {scope, empty, trim, folders}` (Route::Args). Scratch probe (deleted): empty layer removed; followed empty kept; mask with masked child kept; folder of empty children removed (3 layers + 1 folder); 0-10 + blanks to 40 vs content to 30 → 30 (10 frames); audio_guide kept; preview == applied; second run 0; one undo restores stage; whole scene cleans a symbol (touched 1); agent call ok, bad scope refused. ep1 (`scratchpad/produce/p5/ep/ep1.buzz`, 131 layers, 256 symbols): timeline 10 layers / 0 frames / 0 folders; whole scene 26 layers / 0 frames / 0 folders / 10 symbols; preview 0.1 ms. Suites: buzz-scene + buzz-doc green, buzz-ui --lib 375 passed. |
| L3 | Arrange Layers + auto-tidy hooks + focus selection | built 2026-10-21, 2 hooks parked: `buzz-scene/src/arrange.rs` — `Scene::arrange_layers(ArrangeOptions{active, delete_empty})`/`arrange_preview` → `ArrangeReport{folders_made, layers_moved, layers_left_in_place, emptied}`, `LayerGroup` (Character `<Cast/name or symbol name>` incl. whole follow tree / Background / FX / Props / Audio pinned bottom). Units = top-level layer, mask/guide run, or folder block (runs and folders move whole, never classified); a move passes a unit only if their extents don't meet (2 pt margin; extent = memoised resolved bounds over every keyframe + every tween frame, followers through `inherited_transform` per frame up the chain; layer/object filters, guide runs, and shadow casters on a lit stage with a shadow light = unbounded); light-anchor layers never move or get passed. Best anchor per group, fixpoint, same-named top-level folders reused, new folders collapsed except the active layer's; L2's empty-layer pass first (active + light anchors protected). `LayerStack::from_rows` (pub(crate)). Command `ArrangeLayers` "Arrange Layers" (Modify ▸ Timeline, count 203), `Editor::arrange_layers` (one "Arrange Layers" step, none when tidy, status line), agent `arrange_layers {}` (Route::Args; already tidy = ok, 0 steps). `Workspace::auto_tidy_layers` (default true) + `focus_folders` (default false); timeline header **Focus** toggle next to Compact; `Editor::focus_folders_if_changed` per frame from `build_ui` (expands ancestors, collapses other top-level folders, edit "Focus folders", consecutive ones coalesce). Hooks live: FLA/XFL open (`App::finish_open_imported`) and FLA/XFL import onto the stage (`App::finish_merge`) → `Editor::auto_tidy_layers` (own step); AI Direct Story (`App::commit_directed`, own step after the dialogue sync); Story Direct (`Editor::story_direct`) and Direct as film (`Editor::direct_sequence_dressed`) → `Editor::auto_tidy_into_step("Direct")`, folded into the Direct step because `ai_agent_coverage::the_story_panel_commands_run_from_args` requires direct_scene = 1 undo step. **Follow-ups done 2026-10-21:** episode/Producer build — `BuildOptions::tidy_layers` default **true**, Arrange runs inside the same "Build Scene N" step; new folders span the timeline (`stack.frame_count()`), so every row reaches the end. New Shot from Setup — auto-tidy stays **off** by decision (setup layers are matched by name to continue/split shots; comment in `Editor::new_shot_from_setup`). Characters widened: a `follows` tree (root + all followers) is one Character if any layer holds a character symbol, or a symbol in library folder `Cast parts`; named after the character symbol, else the common prefix before " part" ("LALTEN part 11" → LALTEN), else the root layer. Verified (scratch probe, ep1.buzz loaded headless, not saved): folders LALTEN 38, MUNNA 38, GUDDU 38, Props 2, Background 1; rows 131 → 126 (10 empty deleted, 4 left in place); order flips among overlapping layers 0; second run 0 moves. `episode_build` 21/21, `sequence_setups` 9/9, `ai_agent_coverage` 41 pass (2 ignored), `buzz-scene` 549/549. |
| L4 | Verify on ep1 document: counts, no paint-order change among overlapping layers, idempotent re-run, suites green | pass 2026-10-21 (scratch probe deleted). Synthetic (10 layers + default layer; Cast/Raju head+arm following a tweened body, glow, tree, bush, rock, sky, empty, sound): folders Raju/FX/Props/Background/Audio, 8 filed, rock left in place (overlaps Raju), 2 empties deleted; overlapping-pair paint-order flips 0 (brute-force per-frame resolved bounds), light bands unchanged, chain in one folder, Audio last, active layer's folder open and the rest collapsed, preview == apply, 1 undo step "Arrange Layers", second run 0 changes / 0 steps, one undo restores the stack exactly. ep1 (`scratchpad/produce/p5/ep/ep1.buzz`, 1 scene, 131 layers, lit with 2 shadow lights, not saved): top-level rows 131 → 5, folders 2 (Props 117, Background 1), 118 filed, 3 left in place, 10 empties deleted, order violations 0, mask groups unchanged, 20.5 ms, second run 0. Same with the lights off (bounds path): rows 131 → 4, Props 118, 2 left in place, violations 0, 218 ms, second run 0. Suites: buzz-scene + buzz-doc green; buzz-ui --lib 375 passed; ai_agent_coverage 40/41 on the full run, the one failure (direct_scene undo steps) fixed by folding and re-run green; episode_build 21/21, sequence_setups 9/9, direct_sequence 6/6, direct_a_story_shows_the_panel 4/4, buzz-app lib "directed" 2/2. |
