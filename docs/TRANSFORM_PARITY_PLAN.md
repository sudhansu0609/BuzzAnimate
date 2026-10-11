# Transform tools — Animate parity plan

Written 2026-10-05. Request: "rotate, scale, drag, Free Transform, Selection and
Subselection still aren't right against Adobe Animate — rotate, transform, skew
etc. — fix it."

## Status

| Step | What | State |
|---|---|---|
| A1 | `Editor::selection_box()` — oriented transform box | done — `selection_box_follows_a_rotated_object`, `fill_and_stroke_share_one_oriented_box`, `mixed_rotations_fall_back_to_the_aabb` |
| A2 | `ToolContext.selection_box`; zones / move-test on the oriented box | done — `oriented_zone_matches_axis_aligned_when_unrotated`, `rotated_box_corner_is_a_corner`, `pressing_the_empty_corner_of_a_turned_boxs_aabb_does_not_move_it` |
| A3 | Scale / side-scale / skew computed in the box's own axes | done — `side_scale_on_rotated_box_stays_rectangular`, `corner_scale_on_rotated_box_holds_opposite_corner`, `skew_on_rotated_box_shears_along_its_own_edge`, `free_transform_side_drag_on_a_rotated_square_does_not_shear_it`, `an_unturned_box_is_scaled_exactly_as_before` |
| A4 | Stage draws handles + rotate ring on the oriented quad; cursors follow | done — chrome in `stage.rs` / `app.rs::transform_cursor` (no render test; `the_rotate_mark_appears_on_the_corner_the_pointer_is_on` updated to the corner-array signature) |
| A5 | Tests for A1–A4; full `buzz-app` suite green | done — lib 563 tests: 561 pass; the 2 timing perf tests (`dragging_a_selection_on_a_heavy_document_stays_interactive`, `dragging_a_lamp_over_a_scene_stays_interactive`) failed under load and pass alone; all 56 integration test binaries pass |
| A-review | Opus review 2026-10-05 | **accepted.** Read `transform_in_frame`, `selection_box`, `affine_frame_of`, stage handles. One change: a frame with no rotation or skew (`c[1]==c[2]==0`) now returns the plain axis-aligned box, so unrotated scales stay float-exact (no `F*T*F⁻¹` round trip). Re-ran 78 transform/box/zone/cursor lib tests: all pass. Not yet checked by eye in the real app. |
| B1 | Subselection: selected-anchor set (click, Shift-click, marquee) | done — `clicking_an_anchor_selects_it_and_shift_click_toggles`, `a_marquee_selects_the_anchors_inside_it`, `the_anchor_selection_is_forgotten_when_the_selection_changes`, `dragging_a_selected_anchor_moves_all_selected_in_one_undo_step` (live: the drag now reshapes the path as it moves; `MoveAnchors`), buzz-geom `moving_several_anchors_*`; tools.rs tests for press/click/marquee. Stage: selected anchors filled, others hollow (no render test) |
| B2 | Subselection: tangent handles shown + draggable (smooth mirrors, Alt breaks) | done — `a_selected_anchor_shows_its_two_handles_and_others_show_none`, `dragging_a_smooth_handle_turns_the_opposite_handle_with_it`, `alt_dragging_a_handle_moves_only_that_handle`, `handles_follow_the_object_transform`, `alt_dragging_a_corner_pulls_symmetric_handles_out`; buzz-geom `smooth_means_collinear_within_two_degrees`, `dragging_a_smooth_handle_turns_*`, `breaking_a_handle_*`, `pulling_handles_from_a_corner_*`, `an_implied_closing_line_is_written_out_*`. Handles drawn in `stage.rs` (no render test) |
| B3 | Subselection: Delete removes anchors, arrows nudge | done — `delete_takes_out_the_selected_anchors_and_is_undoable`, `deleting_all_but_one_anchor_deletes_the_object`, `delete_without_selected_anchors_still_deletes_the_object`, `arrow_nudges_move_the_selected_anchors_and_not_the_object`, `nudging_anchors_is_in_world_units_on_a_turned_shape`, `nudge_still_moves_the_object_without_selected_anchors`; buzz-geom `deleting_*`. Backspace and the 10-unit Shift step are wired in `app.rs::keyboard_commands` (not unit-tested: needs an egui input) |
| B-review | Opus review 2026-10-05 | **accepted.** Re-ran buzz-geom `path_edit` 28/28 and buzz-app lib anchor/handle/nudge/delete/subselection/marquee 51/51. Read `move_handle` (mirror keeps the opposite length, decided at press). Not checked by eye: chrome drawing, Backspace and Shift+arrow (10) keys (compiled only). Known: Alt on an anchor that already has handles is an ordinary drag; QuadTo sides are not pulled out. |
| C1 | Selection tool: corner drag on raw shapes | done — `dragging_a_corner_moves_the_fill_and_the_outline`, `dragging_a_corner_of_a_turned_shape_puts_it_under_the_pointer`, `a_corner_drag_is_one_undo_step`, `a_selected_shape_is_never_bent`, `an_instance_is_never_bent`, `a_locked_shape_is_not_bent`, `a_click_on_an_edge_still_selects_the_shape`, `a_drag_inside_a_fill_still_selects_and_moves_it`, `the_cursor_tells_a_corner_from_an_edge_and_nothing_else`. Editor-owned gesture in `editor/shape_edit.rs` (`ShapeGesture`, absolute from a press-time snapshot, one label = one undo step); cursor in `app.rs::draw_tool_cursor` (Crosshair on a corner; no render test) |
| C2 | Selection tool: segment bend | done — `bending_a_rectangles_top_edge_leaves_its_corners_and_lifts_the_midpoint`, `a_fill_and_its_outline_bend_together`, `bending_is_one_undo_step_and_the_preview_is_the_commit`, `bending_a_turned_and_squeezed_shape_follows_the_object_transform`; buzz-geom `nearest_segment_*`, `bending_*`, `the_control_nearer_the_grab_bends_more` (`bend_segment`, `nearest_segment` in `path_edit`). Edge cursor is `Grab` |
| C3 | Selection tool: Ctrl-drag adds a corner | done — `a_ctrl_drag_on_an_edge_adds_a_corner_and_moves_it`, `a_ctrl_click_on_an_edge_adds_nothing`; buzz-geom `splitting_*` (`split_segment`). The split is made in the press-time snapshot, so a Ctrl-click edits nothing |
| D1 | Free Transform Ctrl+corner = Distort (raw shapes), Shift = taper | done — `ctrl_dragging_a_corner_distorts_that_corner_only`, `shift_ctrl_dragging_a_corner_tapers_symmetrically`, `distorting_a_turned_selection_works_in_its_own_box`, `a_distort_is_one_undo_step_and_the_preview_is_the_commit`, `an_instance_in_the_selection_makes_ctrl_corner_an_ordinary_scale`; buzz-geom `warp.rs` (`subdivide`, `bilinear_warp`, `drag_corner`, `map_points`) with `the_corners_land_on_the_new_corners_exactly`, `an_unmoved_quad_is_the_identity`, `a_taper_moves_the_neighbour_along_the_nearer_edge_the_other_way`. Ctrl is read at the press; Shift live |
| CD-review | Opus review 2026-10-05 | **accepted.** Re-ran buzz-geom 208/208 and buzz-app lib bend/corner/distort/taper/edge/cursor/locked/instance filters 48/48. **By design (Animate parity):** a Selection-tool press on an unselected raw shape's corner/edge now drags that corner / bends that edge instead of moving the object; a tap still selects; interiors still move. Known: Distort subdivides 8× per segment (compounds on repeat); 3D-turned shapes are excluded; cursors (Crosshair/Grab) not checked by eye. |
| E1 | Selection tool = Animate: no handles/ring/pivot, no resize/skew/rotate; outline only | done — tools.rs `the_selection_tool_has_no_handles_and_moves_from_anywhere_in_the_box`; editor `the_selection_tool_moves_from_where_free_transform_has_handles`, `the_selection_tool_has_no_rotate_ring`, `the_selection_tool_does_not_move_a_selected_ring_by_its_hole`, `a_selected_text_is_dragged_by_its_box_not_only_its_ink`, `free_transform_still_has_what_the_arrow_gave_up`, `free_transform_moves_the_transformation_point_and_the_selection_tool_does_not`; chrome (`stage.rs`, via `draw_chrome` shapes) `the_selection_tool_hatches_a_shape_and_draws_no_gizmo`, `free_transform_hatches_a_shape_and_adds_its_gizmo`, `the_selection_tool_outlines_an_instance_and_does_not_hatch_it`. Gizmo (`begins_a_transform`, pivot grab, rotate commit, `pointer_on_pivot`, `transform_cursor`, handles/ring/pivot drawing) is Free Transform only; a Selection press moves only if it is **on a selected object** (the editor hands the machine the box only then; a ring's hole is a marquee; text moves by its box). Subselection is untouched (still has the hidden pivot drag / ring commit). Existing tests changed (the old Selection-tool handles / Free Transform's whole-drawing pick): `selection_corner_scales_when_the_pivot_has_not_been_stored_yet` -> `free_transform_corner_scales_...`, `the_selection_tool_scales_from_a_corner` -> `free_transform_scales_from_a_corner`, `both_transform_tools_rotate_while_the_pointer_moves` -> `free_transform_rotates_while_the_pointer_moves`, `corners_resize_and_edges_skew_from_either_tool` -> `..._from_free_transform`, `the_selection_tool_can_move_the_transformation_point_too` -> `free_transform_moves_the_transformation_point_and_the_selection_tool_does_not`, `free_transform_grabs_the_touching_pieces_of_a_drawing` -> `a_click_picks_only_the_object_under_it_with_either_tool`; now Free Transform only: `turning_something_never_shears_it`, `a_corner_handle_over_another_object_still_scales`, `a_rotation_turns_about_the_moved_transformation_point`, `a_transformation_point_on_a_corner_still_leaves_room_to_rotate`, `a_group_rotates_about_its_moved_transformation_point`; `free_transform_can_resize_a_multi_piece_drawing_twice_in_a_row` takes its pieces by marquee; `touching_text_objects_are_never_grouped_into_one_drawing` asks `connected_pieces`. Lib run: 646 pass + `dragging_a_lamp_over_a_scene_stays_interactive` (timing, passes alone); buzz-geom 208/208 |
| E2 | Raw-shape selection drawn as Animate's dotted hatch, not a box | done — new `hatch.rs` (scanline over the flattened outline on a screen-aligned dot lattice; strokes as dotted lines; one mesh; MAX_DOTS=40000 per frame by widening the pitch; no boolean ops, no cache needed): `a_fill_is_dotted_inside_itself_only`, `a_hole_is_left_undotted`, `a_concave_shape_is_not_filled_across_its_notch`, `the_hatch_stays_inside_the_view`, `a_stroke_is_dotted_along_its_line`, `dots_keep_their_spacing_round_a_corner`, `the_dot_budget_holds_however_large_the_selection`, `degenerate_input_makes_no_dots_and_does_not_hang`; `Editor::drawn_shape` shares the object_quad projection chain. **Measured** (`fifty_selected_shapes_hatch_inside_a_frame`, optimised dev profile): 50 shapes / ~14k dots = 0.10 ms flatten+project, +0.07 ms dot geometry, 0.34 ms mesh, whole chrome frame incl. egui tessellation 1.4 ms vs 0.02 ms unselected. Not checked by eye |
| E3 | Raw-shape picking: click = that fill/stroke only; double-click = fill + its strokes / connected strokes (both tools) | done — `a_click_picks_only_the_object_under_it_with_either_tool`, `clicking_a_fill_or_its_outline_takes_one_of_the_two`, `double_clicking_a_fill_selects_it_and_the_strokes_touching_it`, `double_clicking_a_stroke_selects_every_stroke_connected_to_it`, `double_clicking_a_raw_shape_does_not_leave_the_symbol_being_edited`; `double_clicking_an_instance_opens_it_and_empty_space_leaves` unchanged and green. `drawing_group_of` is replaced by `connected_pieces` (fill: itself + touching strokes; stroke: flood over strokes; text never joins), reached from `enter_or_leave_at`; `PickAt` no longer grows Free Transform's pick |
| E4 | Alt-drag a selection = drag a duplicate (one undo step) | done — `alt_dragging_a_selected_shape_leaves_the_original_and_drags_a_copy` (both tools, one undo step), `alt_dragging_an_unselected_object_copies_that_object`, `an_alt_click_copies_nothing`, `alt_on_a_free_transform_corner_still_scales_about_the_point`, `alt_on_a_shapes_edge_copies_it_instead_of_bending_it`. The copy is made when the pointer is a click slop away (so an Alt-click copies nothing) with label `Duplicate`, shared by the copy and every step of the drag |
| E5 | Free Transform: Alt+rotate turns about the opposite corner | done — tools.rs `alt_rotate_turns_about_the_opposite_corner`, `a_live_alt_rotation_is_about_the_opposite_corner`; editor `alt_rotating_with_free_transform_holds_the_opposite_corner`. In `transform_in_frame`, so preview, live steps and release agree; Shift still snaps 45 |
| E6 | Free Transform: the turned box persists for the selection's session (multi-select too) | done — `a_turn_survives_in_the_box_of_a_mixed_selection`, `the_kept_box_is_dropped_when_the_selection_changes_or_history_moves`, `only_a_free_transform_turn_keeps_a_box`. `BoxSession` (ids, frame, playhead frame, history stamp) in a `RefCell`, validated on every `selection_box()` call; `F = T*F` on each committed step; also dropped by Undo/Redo |
| E7 | Marquee/Lasso over raw shapes cuts them (partial selection) | not started (batch 2) |
| E8 | Envelope modifier | not started (batch 2) |
| E9 | Double-click a group enters it (group edit mode, rest dimmed); double-click empty stage leaves | not started (batch 2): no group edit context exists yet |
| E-review | Opus review 2026-10-06 | **accepted.** Re-ran 81 E-related + moved tests (all pass) and 76 subselection/anchor/handle/selection/free-transform tests. **Added in review:** Subselection lost its hidden transformation-point drag, rotate-ring commit and gizmo preview (Animate's Subselection has none); test `the_subselection_tool_has_no_point_and_no_rotation`. Visible changes: a Selection press on an empty part of a selected object's box marquees; Free Transform needs a double-click to take fill+outline together. Hatch cost ~1.4 ms/frame for 50 shapes (dev profile). Not checked by eye. |

Update this table after each step. Never mark a step done without the test
named for it passing.

## What is wrong today (audited 2026-10-05)

1. **The transform box is always axis-aligned.**
   `Editor::selection_bounds_drawn` (`editor.rs` ~3259) unions every object's
   `object_quad` into one `Rect`. The handles (`stage.rs::draw_selection`
   ~975–1064), the zone test (`tools.rs::transform_zone` ~1863) and the
   scale/skew maths (`scale_about_corner`, `scale_about_side`, `skew_about`)
   all work on that rect.
   Result: rotate a rectangle 30° and its handles jump to the box *around* it;
   then dragging a side handle stretches it along world X, so the rectangle
   **shears into a rhombus**. In Animate the box turns with the object: the
   handles stay on its corners and a side drag stretches along its own width.
   This is the main "rotate/scale/skew isn't right" symptom.
2. **Subselection only drags anchors.** `buzz_geom::path_edit::anchors` skips
   control points on purpose. Nothing tracks which anchor is selected, tangent
   handles are never shown, and a curve can't be reshaped by its handles.
3. **The Selection tool can't bend raw shapes** the way Animate's arrow does
   (drag a segment to bend it, drag near a corner to move it).
4. **There is no Distort** (Ctrl+drag a Free Transform corner).

Already right and **not to be changed**: rotation about the transformation
point; Shift-rotate snapping to 45°; Alt-scale about the point; live preview;
one undo step per drag; the pivot travelling with the artwork; the Selection
tool also exposing resize/skew (the user chose that deliberately); skew about
the transformation point (kept as-is: there is no ground truth that Animate
anchors the opposite edge instead).

## Stage A — the oriented transform box (do first)

### A1. `TransformBox`

In `tools.rs` (shared by tools and editor):

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformBox {
    /// The box in its own (unrotated) space.
    pub local: Rect,
    /// Its own space → the focal plane the pointer works in (the same space
    /// `selection_bounds_drawn` returns).
    pub to_doc: Affine,
}
```

Helpers on it: `corners() -> [Point; 4]` (TL, TR, BR, BL in doc space),
`mid_edges()`, `contains(p)` (inverse-map, then `local.contains`), `center()`,
`aabb()`.

`Editor::selection_box(&self) -> Option<TransformBox>`:

- For each selected object, build its focal-plane quad exactly as
  `selection_bounds_drawn` does (object_quad → shot inverse → unplace). Also
  get its local bounds (the same `resolved_local_bounds` `object_quad` uses).
  Factor that out into one helper so the two paths can't drift apart.
- From the quad, derive the affine `A` that maps the local rect's corners onto
  it: three corners fix it, and the fourth must agree within `1e-6 * size`.
  If it doesn't (a perspective tilt), that object has **no** affine frame.
- **One object, affine frame** → `TransformBox { local: its local bounds, to_doc: A }`.
- **Several objects whose `A`s share a linear part** (all four coefficients
  within 1e-6, which is the usual case for a drawing's fill + stroke after
  being transformed together) → `to_doc` = the first object's `A`. `local` =
  the union of every object's doc quad mapped through `to_doc.inverse()`.
- **Anything else** (mixed rotations, any perspective, a degenerate
  scale-to-zero) → today's behaviour exactly: `local = selection_bounds_drawn()`,
  `to_doc = Affine::IDENTITY`.
- A zero-width or zero-height local rect (a horizontal line) → fall back to the
  AABB as well. A singular matrix gives no usable handles.

`selection_bounds_drawn()` stays and keeps returning the AABB: the pivot
default, snapping and other callers use it. Its result must equal
`selection_box()?.aabb()` up to float noise.

### A2. Tools use the box

- Add `pub selection_box: Option<TransformBox>` to `ToolContext`. Fill it in
  every place `selection_bounds` is filled (`editor.rs` ~1668, ~2059, ~2081,
  ~2178, and the tests' contexts in `tools.rs`; tests may use
  `TransformBox::axis_aligned(rect)`).
- `transform_zone(b: TransformBox, pivot, at, grab)`. Same precedence as
  today (pivot → corner → rotate ring → side handle → edge line → inside), but
  measured **in doc space**:
  - corner distance = distance to `b.corners()`;
  - rotate ring = within `3*grab` of a corner **and** `!b.contains(at)`;
  - side handles = within `grab` of `b.mid_edges()`. The `bool` keeps its
    meaning in **local** terms: `true` = the local top/bottom edge;
  - edge lines = distance to the edge **segment** ≤ grab, excluding `grab` at
    each end (so corners win);
  - inside = `b.contains(at)`.
  With `to_doc = IDENTITY` this must give exactly today's answers. The
  existing `transform_zone` tests must pass unchanged once wrapped with
  `TransformBox::axis_aligned`.
- `begins_a_move` / the "press is inside the selection" test uses
  `b.contains` instead of the AABB, so pressing in the empty corner of the
  AABB around a rotated object no longer moves it. Animate behaves this way.
- `Transforming` stores the box (`TransformBox`) instead of `bounds: Rect`,
  frozen at press time, as `bounds` is today.

### A3. Scale and skew in the box's own axes

In `transform_for` (and in the duplicate paths at `tools.rs` ~919–965
(preview) and ~1195–1292 (release); all three must go through one function):

- **Rotate**: unchanged, computed in doc space (`rotate_about(pivot, …)`).
- **Corner / Side / Edge**: let `F = box.to_doc`, `Fi = F.inverse()`. Map
  `origin`, `end` and `pivot` through `Fi`. Call the **existing**
  `scale_about_corner` / `scale_about` / `scale_about_side` / `skew_about`
  with `box.local` and those local points. The doc transform is
  `F * T_local * Fi`. Corners then stay anchored at the opposite corner of
  the *turned* box, Shift keeps proportions along its own axes, and a side
  handle stretches only along the object's own width or height.
- If `F` is singular, return `Affine::IDENTITY`.

### A4. Stage chrome

`stage.rs::draw_selection`: draw the eight handles at
`box.corners()` / `box.mid_edges()` mapped `to_screen`, not at an egui rect.
Draw the handles as squares/circles that stay screen-aligned (Animate's do).
`draw_rotate_hint` gets the four corner positions instead of a `Rect`.
The per-object outlines stay as they are.

Cursor (`editor.rs` ~1970 and wherever the resize cursor icon is chosen):
pick the resize cursor from the handle's direction from the box centre in
screen space, snapped to the nearest of the four egui diagonal/axis resize
icons. The skew cursor follows the edge's direction the same way.

### A5. Tests (headless: `Editor` + `ToolMachine` only, no real mouse, no renders)

In `tools.rs` tests:
- `oriented_zone_matches_axis_aligned_when_unrotated`: for a grid of points,
  `transform_zone(TransformBox::axis_aligned(r), …)` equals the old answer.
- `rotated_box_corner_is_a_corner`: a box rotated 30°; a press on its rotated
  TR corner gives `Corner`, and a press on the AABB's empty corner gives
  `Rotate` or `Inside`, never `Corner`.
- `side_scale_on_rotated_box_stays_rectangular`: rotate 30°, drag the local
  right side handle outward by 50 along the box's local x. The resulting
  `F' = T * F` has orthogonal columns (dot < 1e-9), the local-x column grows
  by the expected ratio, and the local-y column is unchanged.
- `corner_scale_on_rotated_box_holds_opposite_corner`.
- `skew_on_rotated_box_shears_along_its_own_edge`.

In `editor.rs` tests:
- `selection_box_follows_a_rotated_object`: draw a square, rotate 30° via
  `TransformSelection`. `selection_box()` has a rotated `to_doc` whose corners
  equal `object_quad` (mapped to the focal plane).
- `fill_and_stroke_share_one_oriented_box`: a drawing whose pieces share a
  transform gives one oriented box.
- `mixed_rotations_fall_back_to_the_aabb`.
- End-to-end: Free Transform press/drag/release on the rotated square's right
  side handle → the object's transform is still a similarity times a
  non-uniform scale in its own axes (no shear).

Run: `cargo test -p buzz-app` (and `-p buzz-ui` if touched). **Never run
`cargo fmt`.** Existing tests must stay green. If an existing test encoded the
old AABB behaviour on a *rotated* selection, update it and say so in the
report.

## Stage B — Subselection parity (after A is reviewed)

- **B1** `Editor` keeps `selected_anchors: Vec<usize>` (element indices) for
  the single sub-selected shape. It is cleared when the selection changes.
  Click an anchor = select only it. Shift-click toggles. A marquee with the
  Subselection tool selects the anchors inside it. Dragging any selected
  anchor moves **all** selected ones together (one undo). Selected anchors
  are drawn filled, the rest hollow.
- **B2** For each selected anchor, show its tangent handles: the incoming
  control point (`c2` of its own CurveTo / `c` of its QuadTo) and the
  outgoing one (`c1` of the next CurveTo), each with a line to the anchor.
  Dragging a handle moves that control point. If the anchor is **smooth**
  (both handles collinear within 2° at press time), the opposite handle turns
  with it and keeps its length. **Alt-drag** moves one handle only, breaking
  the point into a corner. **Alt-drag on a corner anchor** with no handles
  pulls out symmetric handles (LineTo neighbours become CurveTo). New
  `buzz_geom::path_edit` functions, unit-tested there.
- **B3** Delete/Backspace with anchors selected removes them (the path
  re-joins its neighbours; a path left with fewer than 2 anchors deletes the
  object). Arrow keys nudge the selected anchors by 1 px, or 10 px with Shift.

## Stage C — Selection tool bends raw shapes (Animate's arrow)

Applies only to **raw shapes** (not instances, groups, text, armatures) on an
unlocked, visible layer at the current frame, and only when the press is
**not** on an already-selected object (that stays a move, as today).

- **C1 corner drag.** A press within `ANCHOR_GRAB_PX/zoom` of an on-curve
  anchor of any such path, then a drag: that anchor moves, with its handles
  (reuse `buzz_geom::path_edit::move_anchors`). **Every** raw path with an anchor
  within reach of the press point moves the same way, so a fill and its outline
  (separate objects here, coincident in the drawing) move together.
- **C2 segment bend.** Otherwise, a press within `grab` of a segment: find the
  nearest `t` on it (clamped to [0.1, 0.9]). A LineTo/QuadTo segment is promoted
  to the equivalent CurveTo. The drag delta `d` moves the controls by
  `Δc1 = d·(1−t)·k`, `Δc2 = d·t·k` with
  `k = 1 / (3·t·(1−t)·((1−t)² + t²))`, so the curve passes through the pointer
  at `t` and the near side bends more. **Absolute from the press**: the controls
  are computed from the segment as it was at press, so the preview equals the
  commit. Every raw path with a segment within reach of the press point bends,
  for the same fill-plus-outline reason. A new pure function in
  `buzz_geom::path_edit` (`bend_segment(path, element, t, delta)`), unit-tested:
  the point at `t` lands on `B(t)+d`, the end anchors don't move, and a line
  becomes a curve.
- **C3 Ctrl-drag on a segment** splits it at `t` (de Casteljau), inserting a
  corner anchor, then drags that anchor as in C1.
- **Paths live in object space.** Map the press and the delta through the same
  inverse drawn transform the Subselection drags use (see
  `handles_follow_the_object_transform`).
- **One undo step per drag.** Use the live-action + collapse pattern Stage B
  used for `MoveAnchors`.
- **Cursor.** Over a bendable segment of an unselected raw shape, show a
  distinct cursor (`egui::CursorIcon::Grab` is acceptable if there's no arc
  cursor); over an anchor, `Crosshair`. Unchanged everywhere else.
- **Tap without drag** still selects (today's behaviour). A drag that starts
  inside a fill but not near an edge still selects and moves it, as today.
- Tests (Editor + ToolMachine): bending a rectangle's top edge leaves its
  corners fixed and lifts the midpoint to the pointer; fill and outline bend
  together; dragging a corner moves both; Ctrl-drag adds an anchor; a selected
  shape still moves; an instance is never bent; one undo step restores.

## Stage D — Distort (Free Transform, Ctrl+corner)

- Only when **every** selected object is a raw shape; otherwise Ctrl+corner is
  an ordinary corner scale.
- At press, snapshot each selected path (object space) and the oriented box
  quad `Q` (`selection_box().corners()`). Dragging corner `i` gives `Q'` with
  only corner `i` moved. **Shift (taper)**: the corner sharing the nearer edge
  (the one most parallel to the drag) moves by the mirror of the drag along
  that edge.
- The warp `W` is the bilinear map from `Q` to `Q'`. Each path is first
  subdivided (every segment split into 8 cubic pieces by de Casteljau) and then
  every point, anchors and controls alike, is mapped:
  `p' = T⁻¹(W(T p))`, where `T` is the object's drawn transform. Always from
  the snapshot, never compounded, so preview = commit. Pure helpers in
  `buzz_geom` (`subdivide`, `bilinear_warp`), unit-tested: the corners land on
  `Q'` exactly, and an unmoved quad is the identity.
- One undo step per drag (same pattern as C).
- Tests: Ctrl-drag the top-right corner of a square: TR lands on the pointer
  and the other three corners stay; Shift tapers symmetrically; an instance in
  the selection falls back to scaling; one undo step restores.

## How to verify (Opus review)

- Read the diff. Re-run `cargo test -p buzz-app`. Check that the A5 tests
  assert what they claim (orthogonality, held corner).
- Headless `App::new` + `build_ui` check that a rotated selection draws its
  handles on the rotated corners (inspect the painter's shapes, don't render).

## Stage E — "exactly like Animate" (user, 2026-10-06)

The user asked for the Selection and Free Transform tools to work **exactly**
like Animate's. That overrides the earlier choice (not the user's; a design
call in code) to give the Selection tool transform handles.

- **E1 Selection tool.** No resize handles, rotate ring, skew zones or
  transformation point. A press inside a selected object moves it; nothing else
  on the box does anything. A selected instance/group/text/armature shows a
  thin solid outline on its oriented quad (`object_quad`), as now, but solid
  instead of dashed. The C1–C3 shape edits (bend, corner drag, Ctrl corner) are
  Animate's arrow and **stay**. Free Transform keeps everything it has. Update
  the tests that assert the Selection tool resizes/skews/rotates
  (`corners_resize_and_edges_skew_from_either_tool` and any similar) so they
  assert it for Free Transform only and assert *move* (or nothing) for Selection.
  List every test changed and why.
- **E2 Hatch.** A selected raw shape (fill or stroke object) is drawn with a
  dotted hatch over its own geometry (Animate's selection pattern), not a box,
  under both tools. Free Transform additionally shows its box. Must stay cheap
  (no per-frame boolean ops; cache per object+zoom or draw a tiled dot pattern
  clipped by tessellated geometry). Measure: no visible frame cost on a 50-shape
  selection (report ms).
- **E3 Picking (both tools).** Single click on a raw fill selects that fill
  object only; on a raw stroke, that stroke object only. Double-click on a fill
  selects it plus the strokes touching it (`drawing_group_of` logic, strokes
  only); double-click on a stroke selects all strokes connected to it. A
  double-click on an instance/group still enters it (existing). Free
  Transform's "grow the pick to the whole drawing" goes. Shift-click toggles, as
  now.
- **E4 Alt-drag duplicates.** With the Selection or Free Transform tool,
  Alt-pressing on a selected object (or an unselected one, which is selected
  first) and dragging leaves the original in place and drags a copy. It's
  one undo step, and the copy is selected afterwards. Alt on a box corner in Free
  Transform keeps its existing meaning (scale about the point).
- **E5 Alt-rotate.** Free Transform rotate zone with Alt: turns about the
  corner opposite the one the drag started at (in the oriented box), not the
  transformation point. Shift still snaps 45°.
- **E6 Session box.** After a Free Transform rotate/skew of a selection whose
  objects don't share one transform (so `selection_box` falls back to the
  AABB), the box keeps the turn: the editor stores a session frame (`F` and the
  local rect), sets `F = T·F` on each committed transform, and drops it when
  the selection changes, on undo/redo, or on a frame change. `selection_box`
  prefers a live session frame.
- Tests: headless (Editor + ToolMachine), each item named. Never `cargo fmt`.
