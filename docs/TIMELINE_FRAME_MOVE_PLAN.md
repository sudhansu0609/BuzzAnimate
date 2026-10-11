# Timeline — drag selected frames to a new place

Written 2026-10-11. Request (user): "in the timeline I want to click and drag,
or select a few frames and drag them to a new location, and the previous
frames are made blank keyframes."

Animate's behaviour: press inside a frame selection and drag it sideways; on
release the frames land at the new place, overwriting what was there, and the
place they left becomes a blank keyframe.

## What exists (checked 2026-10-11)

- Frame selection: click, Shift-click and a drag rectangle →
  `TimelineResponse::select_cells` (`buzz-ui/src/timeline_panel.rs` ~2605–2690,
  `FrameVerticalDrag`); `TimelineSelection {layers, first_frame, last_frame,
  anchor_*}` in `buzz-ui/src/selection.rs:56`.
- Cut/Copy/Paste Frames (`editor.rs` ~9659–9760) copy one frame's objects per
  layer. They do **not** carry keyframe structure, tweens, labels or sounds, so
  they cannot be used for a move.
- `LayerTimeline` (`buzz-scene/src/timeline.rs`): `Keyframe {start, objects,
  label, tween, ease_curve, sound}`, `insert_keyframe` (F6, copies objects,
  no sound), `insert_blank_keyframe`, `from_parts` (sorts, dedups, ensures
  frame 0).
- `Editor::frame_op` (`editor.rs:9599`) — the pattern for an undoable edit over
  the timeline selection (`doc.edit(label, …)`, `scene.update_layer`, skip
  locked layers).

## Status

| Step | What | State |
|---|---|---|
| M1 | `LayerTimeline::move_frames(first, last, to, copy) -> bool` + unit tests | done 2026-10-11 — split for M6 into `segment` (returns `Option<FrameSegment>`: `None` past the end — deviation from `-> FrameSegment`), `blank_range`, `overwrite`; `move_frames` composes them; `cargo test -p buzz-scene --lib timeline` 40 passed; 7 new in `timeline::move_frames_tests` (right into empty, left overwriting, overlap by 1 both ways, mid-span, label/tween/sound travel, copy, refusals). Editor F6 does not bake in-betweens, so the limitation note stays (doc comment) |
| M2 | `Editor::move_frames(to, copy)` over the timeline selection; one undo step; selection + playhead follow | done 2026-10-11 — signature now `move_frames(to, rows, copy)` for M6; with no frame selection it falls back to the playhead frame on the selected/active layers (like `timeline_targets`) so the agent can use it (deviation, not in plan); `cargo test -p buzz-app --lib moving_frames` 2 passed (`editor::tests::moving_frames_over_two_layers_is_one_undo_step_and_skips_locked`, `…moves_the_selection_and_playhead_and_grows_the_document`). Doc frame count is the longest layer, so it grows with no extra call |
| M3 | Timeline drag: press inside the selection and drag → ghost → release moves (Alt = copy) | done 2026-10-11 — `FrameMoveDrag` + `frame_move_drag` in `timeline_panel.rs`, `TimelineResponse::move_frames: Option<MoveFrames{to, rows, copy}>`, routed in `app.rs` as `Command::MoveFrames`. Press read from `press_origin`; Shift/Ctrl presses keep the sweep (choice). Zero-offset release = plain click on the pressed cell. Rows record `ROW_GEOMETRY` temp for headless tests. Tests: `app::frame_move_drag_tests::dragging_a_frame_selection_right_moves_the_frames`, `…a_drag_outside_the_selection_still_selects` (pass). Alt-copy by drag not tested headlessly |
| M4 | AI agent route for the same move | done 2026-10-11 — `Command::MoveFrames { to, rows, copy }` (id `move_frames`, args `{to, rows?, copy?}`, 0-based), `Route::Run` in `agent.rs`, sample added to `ai_agent_coverage.rs`; command round-trip test in `buzz-ui` (count 190→191). `cargo test -p buzz-app --test ai_agent_coverage` 41 passed. Open: the agent has no way to select a frame *range*; it moves the playhead frame on the selected layers |
| M5 | Tests green (buzz-scene, buzz-ui lib, buzz-app lib filters, agent coverage) | done 2026-10-11 — `cargo test -p buzz-scene --lib timeline` 40 passed; `-p buzz-ui --lib` 370 passed (2 ignored); `-p buzz-app --lib timeline` 3 passed; `-p buzz-app --lib -- move frames` 74 passed; `-p buzz-app --lib frame_move_drag_tests` 3 passed; `-p buzz-app --lib -- command --skip every_menu_command_runs` 8 passed; `-p buzz-app --test ai_agent_coverage` 41 passed (1 ignored); `cargo build -p buzz-app` ok. NOT verified: `app::every_menu_command_tests::every_menu_command_runs_and_the_window_still_draws` did not finish in 30 min (killed); not known whether that is new |
| M6 | Drag frames **onto other layers** (vertical move), added by the user 2026-10-11 | done 2026-10-11 — `buzz_ui::frame_move_targets` (shared by ghost and editor) maps display rows; any bad target refuses the whole move (danger-coloured ghost; editor status line, no edit). Editor lifts all, blanks all, writes all; fresh ids per segment when layer changes or copying. Tests (pass): `editor::tests::moving_frames_down_one_layer_gives_fresh_ids_and_keeps_tweens`, `…moving_two_layers_down_one_row_lets_the_middle_layer_give_and_receive`, `…copying_frames_onto_another_layer_leaves_the_source`, `…a_locked_or_folder_target_refuses_the_whole_move`, `app::frame_move_drag_tests::dragging_a_selection_down_a_row_moves_it_onto_the_next_layer`. Locked *source* layers are skipped (as M2), not refused |
| review | Review: rerun tests, read the diff | **accepted, 2026-10-11.** Read `LayerTimeline::segment/blank_range/overwrite/move_frames`, `Editor::move_frames`, `frame_move_targets`, `frame_move_drag` and the cross-layer tests (fresh ids unique across layers, one id per object within a segment, tween still pairs). Reran: buzz-scene lib 549/0; buzz-ui lib 369/1 — the 1 is `shift_click_sweeps_a_range_and_ctrl_click_toggles_one`, which also failed once before this work and passes 3/3 alone (it finds rows by theme colour, shared state across parallel tests): flaky, not this change; buzz-app `frame_move_drag_tests` 3/0, `moving_` 11/0, `frames` 23/0, `timeline` 3/0, `symbol_set` 2/0; `ai_agent_coverage` 41/0. Not run: `every_menu_command_runs_and_the_window_still_draws` (>30 min; `MoveFrames` is not in its command list, so it cannot be affected by the new command). Known: Alt-copy and the red refused ghost have no click-through UI test (editor tests cover both); object re-id is top-level only, like Paste. Not checked by eye in the real app. |

## Design

### M1 — the move, on one layer (`buzz-scene/src/timeline.rs`)

`pub fn move_frames(&mut self, first: u32, last: u32, to: u32, copy: bool) -> bool`
— frames `first..=last` (inclusive) end up at `to..=to + (last − first)`.
`false` (and no change) when `to == first`, or `first` is past the layer's end.
`last` is clamped to the layer's last frame.

1. **Cut the segment out of a clone.** On a clone of `self`, `insert_keyframe(first)`
   and, when `last + 1 < length`, `insert_keyframe(last + 1)`, so the range is
   bounded by keyframes. The segment is every keyframe with `start` in
   `first..=last`, kept whole (objects, label, tween, ease_curve, sound),
   re-based to start at 0; its length is `last − first + 1`.
   - A keyframe synthesised at `first` follows F6: same objects, no sound, no
     tween. (Known limitation: a range starting in the middle of a tween takes
     the tween's start drawing, not the in-between pose. If the editor's F6
     path already bakes the in-between, use that instead and drop this note.)
2. **Leave a blank keyframe behind** (skip when `copy`). On `self`:
   `insert_keyframe(last + 1)` when `last + 1 < length` (so whatever follows
   the range is kept), remove every keyframe with start in `first+1..=last`,
   and make the keyframe at `first` a blank one (`Keyframe::new(first)`: no
   objects, label, tween, ease or sound). Result: one blank keyframe spanning
   the vacated frames.
3. **Overwrite the destination.** With `n = last − first + 1` and `end = to + n`:
   `insert_keyframe(end)` when `end < length` (what follows the destination is
   kept), remove keyframes with start in `to..end`, insert the segment's
   keyframes at `start + to`, extend `length` to at least `end`. Rebuild with
   `from_parts` semantics (sorted, deduped, frame 0 present).
   Overlapping source and destination are fine: the segment is taken before
   either write, and the destination is written last, so moved frames win.
4. Frames before `to` are untouched apart from being cut at `to`, as Animate
   does.

Unit tests (in `timeline.rs`): move right into empty frames (layer grows;
source becomes one blank keyframe; frame after the source keeps its drawing);
move left overwriting; overlapping move by 1 both ways; a range starting
mid-span (keyframe synthesised; the frames before it keep the old drawing);
labels, tweens and a sound on a keyframe inside the range travel with it;
`copy` leaves the source unchanged; `to == first` and `first` past the end
refuse.

### M2 — over the selection (`buzz-app/src/editor.rs`)

`pub fn move_frames(&mut self, to: u32, copy: bool)` — uses
`timeline_selection` (layers + `first_frame..=last_frame`); same offset for
every selected layer. One `doc.edit("Move Frames" | "Copy Frames", …)`.
Skips locked layers and layers whose frames cannot be edited, exactly as
`frame_op` does (sound layers **are** included: a keyframe can carry a sound,
and moving dialogue is a main use). Afterwards the timeline selection covers the
destination, and the playhead goes to `to`. The document's frame count grows if
a layer now runs past it (check how `set_frame_count` / layer lengths relate and
follow that). Status line: "Moved N frames on M layers" / "Copied …".

### M3 — the drag (`buzz-ui/src/timeline_panel.rs`)

- A press on a frame cell **inside the current `timeline_selection`** (its
  layer is selected and its frame in `first..=last`) starts a **move drag**
  instead of the selection drag. A press anywhere else keeps today's behaviour
  (select / sweep).
- While dragging: the frame offset is `pointer frame − press frame`, clamped so
  the destination never starts before frame 0. Draw a ghost (outlined
  rectangle in the accent colour) over the destination cells on every selected
  row, and leave the source highlighted. Hover text / cursor: grabbing.
- On release with a non-zero offset: `TimelineResponse::move_frames =
  Some(MoveFrames { to, copy })`, `copy` = Alt held at release. Zero offset is
  a plain click (today's click behaviour stands).
- The app routes `move_frames` to `Editor::move_frames`, the same way
  `select_cells` is routed (`app.rs` ~6310).
- Must not break: Shift/Ctrl click, the drag rectangle, ruler scrubbing,
  layer drag-reorder in the name column, the right-click menus.

### M4 — AI agent

User rule: the AI can do everything the user can by hand. Expose the move the
way the agent already reaches timeline frame operations (`agent.rs` lists
`CutFrames | CopyFrames | PasteFrames | ClearFrames`); if that needs a new
`Command` with data, add `Command::MoveFrames { to, copy }` and keep the
command tables/coverage tests consistent.

### M5 — tests

- M1 unit tests above.
- Editor: move over two layers is one undo step; a locked layer is skipped;
  selection and playhead follow; the doc frame count grows when needed.
- Headless timeline test (one `egui::Context`, events fed through
  `App::build_ui`): select frames 2–4 on a layer, press inside the selection,
  drag 3 cells right, release → frames 5–7 hold the drawings, frame 2 is a
  blank keyframe; and a press outside the selection still makes a selection
  rather than moving anything.

### M6 — onto other layers (user, 2026-10-11: "do this and implement")

The drag moves frames up/down to other layers as well as sideways.

- **Rows.** The drag has a row offset `dy` as well as the frame offset. Rows
  are the timeline's displayed layer rows, in display order (the same order
  the drag-select rectangle uses). Each source layer maps to the row `dy`
  below/above it, so a two-layer selection moved down one row lands on the
  next two rows.
- **Valid targets.** Every target row must exist and be a layer that holds
  frames and is not (effectively) locked — no folders, no camera or light
  rows. If any target is invalid, the ghost is drawn in the error/warning
  colour and release does nothing except a status line saying why.
- **Order of work, so layers can trade places.** Split the M1 move into three
  `LayerTimeline` calls and keep `move_frames` as their composition on one
  layer: `segment(first, last) -> FrameSegment` (read-only clone of the
  range's keyframes, re-based to 0, plus its length), `blank_range(first,
  last)` (step 2 of M1), `overwrite(to, &FrameSegment)` (step 3 of M1). The
  multi-layer move then: (1) takes every source's segment, (2) blanks every
  source (unless copy), (3) writes every segment to its target. A move down by
  one across three adjacent layers is therefore correct.
- **Object ids.** An object id must not end up on two layers (`find_object`
  would return the wrong one). When the target layer differs from the source,
  or when copying, give the segment's top-level objects fresh ids with
  `scene.next_object_id()` — one old→new map **per segment**, so the same
  object on two keyframes of the segment (F6 copies, tween ends) keeps one id
  and tweens still pair up. Same-layer moves keep their ids (as M1).
- **Selection and playhead** follow to the target layers and `to`.
- **Agent:** the M4 route takes the row offset too (or a target layer for the
  anchor row) — keep it one command.
- **Tests:** scene/editor: move one layer's frames down one layer (source
  blank keyframe, target overwritten, ids fresh and unique across layers,
  tween between two moved keyframes still pairs); two layers down by one
  (overlap: the middle layer both gives and receives); copy across layers;
  a locked or folder target refuses the whole move with no change. Headless
  UI: drag a selection down one row and right two cells → lands on the next
  layer.

## Not in scope (this round)

- Dragging the end of a span to stretch it (Animate's span-end drag).
- Moving camera keys or light keys with the selection.
