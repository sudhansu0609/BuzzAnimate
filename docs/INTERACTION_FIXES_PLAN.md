# Interaction fixes: layer range select, folder collapse, drag/pan speed, Free Transform (2026-09-27)

User report: "I am not able to shift plus select the layers all at once, fix
it. The folder in which I put layers shall be able to be collapsed and
expanded. The character drag and drop, the panning across is too slow; make
it fast and smooth. The Free Transform tool is not reliable: resize, rotate,
move are not reliable; once it resizes once, it just moves etc. Fix it
properly."

## Status

| # | Item | Owner | Status |
|---|---|---|---|
| I1 | Shift+click selects the whole range of layers (timeline names + Layers panel); Ctrl/Cmd+click toggles one | Impl L | DONE — `crates/buzz-ui/src/selection.rs` (range anchor, `layer_range`, `expand_collapsed_folders`), `crates/buzz-ui/src/timeline_panel.rs` (`layer_row` click handling, `TimelineState::range_anchor`, `TimelineResponse::set_range_anchor`), `crates/buzz-ui/src/panels.rs` (Layers panel click handling), `crates/buzz-app/src/app.rs`/`editor.rs` (`apply_timeline`, `Editor::range_anchor`/`set_range_anchor`); tests: `selection::tests::{selecting_a_layer_sets_the_range_anchor, layer_range_is_inclusive_both_directions, layer_range_with_a_missing_anchor_falls_back_to_the_click, expand_collapsed_folders_pulls_in_hidden_descendants, expand_collapsed_folders_leaves_expanded_folders_alone}`, `timeline_panel::tests::shift_click_sweeps_a_range_and_ctrl_click_toggles_one` (response-path, real clicks) |
| I2 | Folder collapse/expand (disclosure triangle on folder rows, timeline + Layers panel) | Impl L | DONE — `Layer.collapsed` already existed in `crates/buzz-scene/src/layer.rs`/`crates/buzz-doc/src/serial.rs` (no schema change, no FORMAT_VERSION bump needed); added `LayerStack::is_row_hidden`/`visible_row_order`, disclosure triangles + row-skip in `timeline_panel.rs` and `panels.rs`, `TimelineResponse::toggle_collapsed` wired through `apply_timeline` as an undoable `doc.edit`; tests: `layer::tests::{collapsing_a_folder_hides_rows_but_not_the_stage, collapsing_a_folder_hides_nested_grandchildren}`, `serial::tests::a_folders_collapsed_state_round_trips`, `app::idle_tests::a_folders_collapsed_state_toggles_through_the_response_path` |
| I3 | Free Transform reliability (repro-first) | Impl T | DONE — root cause: `crates/buzz-app/src/app.rs` `handle_stage_input`'s `response.drag_started()` branch called `Editor::pointer_down_at` at `PointerState::interact_pos()` — the pointer's *current* position once egui has already decided a drag is under way — rather than `PointerState::press_origin()`, the pixel the button actually went down on. Because the stage response senses both clicks and drags (`Sense::click_and_drag()`), egui withholds `drag_started()`/`dragged()` until the pointer has moved past its own click/drag threshold (`max_click_dist`, 6 screen points), and by then `interact_pos()` can already be outside the Free Transform gizmo's corner grab radius (`TRANSFORM_GRAB_PX`, 8 screen px) — especially on an inward (shrink) drag, which moves toward the selection's interior and reads as `TransformZone::Inside` → a move instead of a scale. This is exactly "once it resizes once, it just moves": the corner press itself was never told to the editor at the point it happened. `tools.rs`'s `ToolMachine`/`transform_zone`/zone-vs-cursor logic and `editor.rs`'s bounds/pivot recomputation were re-verified correct (fresh, non-stale, agree with `App::transform_cursor`) and needed no change. Fix: use `press_origin()` instead of `interact_pos()` in that one branch. Files: `crates/buzz-app/src/app.rs` (fix + new `mod free_transform_dispatch_tests` in `idle_tests`), `crates/buzz-app/src/editor.rs` (new tests only). Tests: `app::idle_tests::free_transform_dispatch_tests::a_corner_drag_across_eguis_click_threshold_still_resizes` (fails before the fix — pure translation, no scale; passes after), `editor::tests::{free_transform_can_resize_a_corner_twice_in_a_row, free_transform_can_resize_then_rotate_from_the_rings_new_position, free_transform_can_rotate_then_resize_the_turned_corner, free_transform_resize_is_consistent_across_zoom, free_transform_can_resize_a_multi_piece_drawing_twice_in_a_row, cursor_zone_matches_the_press_after_a_resize}` (all passed even before the fix, confirming the editor/tools layer was already sound; kept as regression coverage). Verified: `cargo test -p buzz-app --lib -- transform` (38 passed), `-- tools::` (58 passed), `-- stays_interactive --test-threads=1` (2 passed), `cargo build -p buzz-app` (clean). |
| I4 | Character drag-and-drop + panning speed (measure-first) | Impl P | DONE — measured first; found no dominant cost left to fix. Panning a 40-part character scene: 0.06 ms/frame (debug), 0.04 ms/frame (release). Panning a 400-part heavy scene: 0.28 ms/frame (debug), 0.22 ms/frame (release). Dragging a placed character instance, 20 moves: 0.22 ms/frame (debug), 0.06 ms/frame (release). Dragging an asset from the Cast/Assets panel over the stage: 0.000 ms/frame while hovering (the window's DnD branch in `App::handle_stage_input` does no scene work per move — a cursor icon and a ghost circle only); the one-time drop (`Editor::place_asset_at`) for a 40-part asset: 5.9 ms (debug), 2.0 ms (release). Root cause already fixed by prior work on this branch (commit 7d953bf and the `SymbolTable` revision-guard note in `crates/buzz-render/src/document.rs`): panning is free because `SymKey` deliberately excludes the camera anchor, and dragging no longer rebuilds the whole symbol table per pointer move because `SymbolTable::refresh` keys on the library's content id, not the scene revision, which no longer bumps on every move. No further fix applied — inventing one where measurement shows none is needed would be scope creep. Files: `crates/buzz-app/src/editor.rs` (four new perf tests only; no non-test code changed). Tests added: `editor::tests::{panning_a_character_scene_stays_interactive, panning_a_heavy_scene_stays_interactive, dragging_a_character_instance_stays_interactive, dragging_an_asset_over_the_stage_stays_interactive}`. Verified: `cargo test -p buzz-app --lib -- stays_interactive --test-threads=1` (6 passed, debug and release), `cargo test -p buzz-render` (all suites pass), `cargo check -p buzz-app` (clean). |
| I4b | Space/Hand pan moves too little (user, via question): measure end-to-end ratio at 100%/150% display scale, fix, add a Hand pan-speed option | Impl S | DONE — measured through the real window path (`App` + `build_ui`, real egui event dispatch), not just the unit-level `pan_screen` I4 had already cleared. Root cause: the Space/middle-button pan branch in `App::handle_stage_input` applied `ctx.input(\|i\| i.pointer.delta())` only inside `if response.dragged() { … }`, and egui withholds `dragged()`/`drag_started()` until the pointer has moved past its own click/drag threshold (`max_click_dist`, 6 screen points) — so every frame before that (a real, slow-starting drag can take several) had its motion computed by egui but never read or applied, permanently lost. The Hand tool's own path (`ToolMachine`/`pointer_down_at`, fixed for I3) already anchors from `press_origin()` and was already 1:1 through the real path (measured, not assumed: ratio 1.0000 at every zoom/display-scale combination tested). Fix: the Space/middle-button branch now seeds its own `App::pan_last_screen` from `press_origin()` on `drag_started()` (mirroring `ToolMachine::last_screen`) and walks every `PointerMoved` in the frame's `events` from there, instead of reading one whole-frame `delta()` only on already-recognised-drag frames; `pan_last_screen` resets on `drag_stopped()`. Confirmed independent of display scale and zoom (both cancel out of the screen-space ratio) and unaffected by the pointer leaving the stage rect mid-drag (`response.dragged()` stays true regardless — confirmed via egui's own `interact()`, not just assumed). Added a Tool Options **Pan speed** control (1×/1.5×/2×/3×, default 1×) for the Hand tool, persisted as `Workspace::pan_speed` (serde default, no `LAYOUT_VERSION` bump); it multiplies only the Hand tool's drag (`Editor::pointer_move_at`) and the Space/middle-button drag (`App::handle_stage_input`'s pan branch) — the wheel's `PanView` is built at its own call site and is never touched. Files: `crates/buzz-app/src/app.rs` (`pan_last_screen` field + rewritten pan branch, `ToolOptions` panel call site, `mod pan_ratio_tests`), `crates/buzz-app/src/editor.rs` (`pointer_move_at` applies `workspace.pan_speed` to the Hand tool's `PanView`), `crates/buzz-ui/src/workspace.rs` (`Workspace::pan_speed: f32`, `default_pan_speed`), `crates/buzz-ui/src/panels.rs` (`tool_options_panel` gains a `pan_speed` param, new `hand_properties` control). Ratios (window-path measurement, 300-point drag): Hand tool 1.0000 before and after (no bug there); Space-drag 0.9833 before (5 of 300 points eaten by the click/drag threshold, confirmed against egui directly in an isolated harness) → 1.0000 after; at 150% display scale (300% zoom, 1.5 `pixels_per_point`) 0.9833 before → 1.0000 after (same absolute-point loss, independent of zoom/ppp since neither enters the input-side computation — `pixels_per_point` is read only by the render path, `crates/buzz-app/src/app.rs` ~10294). Tests: `app::idle_tests::pan_ratio_tests::{hand_tool_pans_1_to_1_at_100pct_zoom_one_move_per_frame, hand_tool_pans_1_to_1_at_300pct_zoom_150pct_display_several_moves_per_frame, space_drag_pans_1_to_1_at_100pct_zoom_one_move_per_frame, space_drag_pans_1_to_1_at_300pct_zoom_150pct_display_several_moves_per_frame, middle_button_drag_pans_1_to_1, space_drag_continues_when_the_pointer_leaves_the_stage_rect, pan_speed_option_scales_the_hand_tools_drag, pan_speed_option_scales_a_space_drag_too}`. Verified: `cargo test -p buzz-app --lib pan_ratio_tests` (8 passed), `cargo test -p buzz-app --lib app::` (80 passed, 2 pre-existing ignored), `cargo test -p buzz-ui --lib workspace` (73 passed), `cargo check -p buzz-app` (clean), `cargo check -p buzz-ui` (clean). |

## I1 — layer range selection

Today, `timeline_panel.rs` `layer_row` name click with Shift/Ctrl/Cmd *toggles*
the one layer (`out.select_layers` push/retain). Animate: **Shift+click
selects every layer between the anchor (last plain-clicked layer) and the
clicked one**, in stack order, and Ctrl/Cmd+click toggles one. Keep an anchor
(`TimelineState` / selection model: `selection.rs`), and apply the same rule
in the Layers panel (`panels.rs`). Rows hidden inside a collapsed folder (I2)
are included only when their folder is inside the range. Tests: pure range
function; response-path test that shift-click from row 2 to row 6 selects 5
layers; ctrl toggles.

## I2 — folder collapse/expand

- `Layer.expanded: bool` (default **true**, serde default + skip if true;
  round-trip test; old files load expanded). It's view state saved with the
  document, as Animate does.
- Folder rows in the timeline and the Layers panel get a disclosure triangle
  left of the name. Clicking it toggles `expanded` (one undo-free doc edit is
  fine, or undoable, matching how visibility toggles are applied).
- Children of a collapsed folder, and their descendants, are **not drawn as
  rows** in the timeline or the Layers panel. The folder row shows a compact
  summary of its children's keyframes in the frame grid (Animate shows the
  folder's span), which is optional: at minimum the row stays empty. Stage
  rendering is unaffected (collapse ≠ hide).
- Drag-into-folder (parked F1 code, `LayerStack::move_to`) must still work:
  dropping onto a collapsed folder files the layer inside and keeps the
  folder collapsed.
- Tests: collapsed folder hides child rows (row count), expanding restores
  them, nested folders, serial round-trip.

## I3 — Free Transform reliability (repro first, no guessing)

Symptom: "once it resizes once, it just moves"; rotate/resize/move are
unreliable. Suspects to check (none confirmed yet):
- Last commit `7d953bf` ("instant drag-to-move, whole-drawing Free Transform
  pick, corner resize fix") changed the press handling. An "inside the
  selection → move" test that runs before the handle/zone test, or a pick that
  grows the selection to the whole drawing after the first gesture, would give
  exactly this.
- Handle and zone geometry computed from a stale box: `tools.rs`
  `transform_zone`, ctx.selection_bounds (`Editor::selection_bounds_drawn`),
  pivot, and `bounds_cache` / Scene `bounds_cache` invalidation after a
  transform edit.
- Gesture state not reset after release (`ToolMachine` gesture back to Idle;
  any "transforming" flag).
- Zone radii in screen pixels versus doc units at different zooms.
Method: write headless tests that drive `Editor` + `ToolMachine` exactly as the
window does (pointer down/move/up in screen coords at a few zooms, a rotated
object, a symbol instance, a raw drawing, a group). Sequences: resize corner →
resize corner again; resize → rotate; rotate → resize; move → resize; edge
squeeze; skew. Each must perform the gesture the cursor promises
(`App::transform_cursor` uses the same zones). Find the root cause, fix it,
and keep all the tests.

## I4 — drag-and-drop and panning speed (after VRAM)

Measure first: frame time while (a) dragging a character (symbol instance)
from the Cast/Assets panel onto the stage, (b) moving a placed character on
the stage, (c) panning the view (Hand / space-drag / scroll) on a typical
character scene and on a heavy import. Find where the time goes (re-encode of
the whole stage per pointer move, cache invalidation on pan, thumbnail
re-render during DnD, bounds recomputation). Fix the dominant cost. Add
`stays_interactive`-style perf tests with budgets.

## Review log

- 2026-09-27 Opus, I1+I2: accepted (buzz-ui 360, layer 35, buzz-doc 213 single-threaded; response-path tests pass). `Layer.collapsed` already existed; no format bump.
- 2026-09-27 Opus, I3: accepted. Root cause is in the window glue, not the tool: the stage's drag branch sent `pointer_down` at `interact_pos()` (where the pointer is once egui's 6 pt click/drag threshold is crossed) instead of `press_origin()`, so an inward corner drag began outside the 8 px grab radius and read as a move. One-line fix in app.rs (~6598) — it serves every tool's drag start, so press accuracy improves for all tools. Re-ran `-- free_transform --test-threads=1`: 12 pass, including the app-level repro that failed before.
- 2026-09-27 Opus, I4: implementer measured encode-only costs (pan 0.06–0.28 ms, drag 0.12 ms, DnD hover ≈0) — no code changed, 4 perf tests added. Not the user's complaint: user clarified (question) that Space/Hand panning **moves too little**. Code reads 1:1 (`pan_screen(delta/zoom)`, no clamp). Follow-up I4b: measure the real window path (App + build_ui, pixels_per_point 1.0 and 1.5, Hand tool and Space) and fix any ratio ≠ 1; add a Hand pan-speed option.
- 2026-09-27 Opus, I4b: accepted. Hand tool was already 1:1 through the real window path (all zooms, 150% display). Space/middle-button pan lost pre-threshold motion (ratio 0.983) — now tracks its own last position from press_origin (1.0). New Workspace::pan_speed (1×–3×) in Hand tool options, applied to Hand/Space/middle drags, not the wheel. pan_ratio_tests 8, app:: 80, workspace 73 pass.
