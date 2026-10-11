# Timeline: lights keyframes, folders, camera layer, header toggles (2026-09-27)

User request (paraphrased): light positions keyframed and tracked on the timeline
like other objects; black text in the dark theme; frame right-click menu's
selection follows the pointer; timeline does not scroll with the playhead during
playback; lock/hide/outline-all in the timeline header; add/lock/hide camera
layer from the header like Animate; layer folders with drag in/out, folder
hide/lock.

## Status

| # | Item | Owner | Status |
|---|---|---|---|
| T0 | Dark-theme black text (`.strong()` → accent_text) | Opus | DONE — `theme.rs` active/hovered fg = `Palette::text()`; test `strong_text_reads_on_the_panel` |
| L1 | Light gizmos + hit-test use the keyed state at the playhead | Impl A | DONE — `buzz-light/src/lib.rs` (`LightRig::keyed_at`), `buzz-app/src/lights.rs` (`gizmos_at`/`target_at_frame` built on `keyed_at`), `stage.rs` gizmo loop, `editor.rs` `begin_light_gesture`; tests: `gizmo_of_a_keyed_lamp_sits_at_the_interpolated_position_mid_span` |
| L2 | Stage drag of an animated light is live and keys at the playhead | Impl A | DONE — `buzz-app/src/lights.rs` (`drag_light_at`, `key_after_edit`), `editor.rs` (`pointer_move_at`/`pointer_up_at` both call `drag_light_at`); tests: `dragging_an_animated_lamp_moves_it_live_and_keys_only_the_dragged_frame` (lights.rs), `dragging_an_animated_lamp_on_the_stage_is_live_and_keys_the_playhead` (editor.rs) |
| L3 | Auto Keyframe on: first drag of an un-keyed light creates keys | Impl A | DONE — `buzz-app/src/lights.rs` `drag_light_at`'s `auto_key` branch; tests: `drag_with_auto_key_on_an_unkeyed_lamp_yields_keys_at_frame_zero_and_the_playhead`, `drag_with_auto_key_at_frame_zero_writes_a_single_key` |
| L4 | Light panel shows/edits keyed state; edits on an animated light key at playhead | Impl A | DONE — `buzz-app/src/app.rs` `fn light_panel` (passes `keyed_at(current_frame)` to `buzz_ui::light_panel`, `response.changed` → `crate::lights::apply_light_edit`), `buzz-app/src/lights.rs` `apply_light_edit`; tests: `applying_a_panel_edit_to_an_animated_light_keys_the_playhead`, `applying_a_panel_edit_with_auto_key_on_an_unkeyed_light_starts_a_track` |
| M1 | Frame/light/ruler context menus: target captured when the menu opens | Impl B | CODE LANDED, UNTESTED — implementer stopped by user 2026-09-27 before tests; `cargo build -p buzz-app` passes |
| P1 | Timeline follows the playhead (playback, stepping) | Impl B | CODE LANDED, UNTESTED — implementer stopped by user 2026-09-27 before tests; `cargo build -p buzz-app` passes |
| H1 | Header: painted show/lock/outline-all icons with tooltips | Impl B | CODE LANDED, UNTESTED — implementer stopped by user 2026-09-27 before tests; `cargo build -p buzz-app` passes |
| C1 | Header: Add/Remove Camera button; camera row eye + lock switches | Impl B | CODE LANDED, UNTESTED — implementer stopped by user 2026-09-27 before tests; `cargo build -p buzz-app` passes |
| F1 | Folder button readable; drag layers into/out of folders | Impl B | CODE LANDED, UNTESTED — implementer stopped by user 2026-09-27 before tests; `cargo build -p buzz-app` passes |
| F2 | Folder eye/lock/outline cascades to contents; "Move Out of Folder" | Impl B | CODE LANDED, UNTESTED — implementer stopped by user 2026-09-27 before tests; `cargo build -p buzz-app` passes |
| X1 | Touching text objects select/move separately | Impl C | DONE — `editor.rs` `drawing_group_of` (text is never a flood seed and never swept in); test `touching_text_objects_are_never_grouped_into_one_drawing` |
| X2 | Per-character styling (select chars in Properties text box → bold/italic/size/colour/font) | Impl C | DONE — `buzz-scene/object.rs` (`TextRun`, `TextData::runs`, `restyle_runs`, `shift_runs_for_edit`), `buzz-text/lib.rs` (`TextSpan`, `outline_runs`), `buzz-doc/serial.rs` (`TextRunDto`, format v45), `buzz-app/editor.rs` (`rebuild_text_object`, `set_text_runs`, `base_text_color`, `partition_text_runs`, `set_text_styled` carries runs via `shift_runs_for_edit`), `app.rs` Properties text section (`TextEdit::multiline` cursor_range, selection remembered in egui temp data, Bold/Italic/size/font/colour apply to the range via `restyle_runs`+`set_text_runs` when one exists); tests: buzz-scene `object::text_run_tests::*` (8), buzz-text `style_tests::a_single_uniform_span_matches_outline_styled`/`two_spans_lay_out_side_by_side_each_tagged_with_its_own_index`, buzz-doc `per_character_styling_runs_survive_the_round_trip`, buzz-app `styling_a_range_of_text_converts_it_to_a_group_and_back`/`a_runs_colour_paints_only_its_own_segment`. The app.rs widget wiring itself has no dedicated egui-harness test (verified by build + the editor-level tests it calls into). |
| W1 | Properties/Effects/Tool + Library/Assets/Cast as ONE tab stack; "Props" tab reads "Properties" | Impl W | DONE — workspace.rs + follow-up app.rs (dock_rects clamped to window); `cargo test -p buzz-app --lib app::` 65 pass, `buzz-ui --lib workspace` 73 pass |
| E1 | Eraser ring size ≠ erased size | Opus | DONE (verified: `cargo test -p buzz-app --lib eraser` 3/3) — ring used `stroke_width` not `eraser_size` (app.rs cursor); ring now scaled by `Editor::edit_scale_on_screen` (view zoom × camera); `erase` cut in edit space was applied to object-local paths without inverting `o.transform` (wrong size/place on any moved/scaled shape) + offcuts lost the transform. Test `the_eraser_cuts_a_transformed_shape_where_it_was_drawn` |
| Z1 | Brush + eraser option "Zoom size with stage" | Impl Z | DONE — `buzz-ui/src/style.rs` (`DrawStyle::brush_zoom_size`/`eraser_zoom_size`, default true), `buzz-ui/src/panels.rs` (checkboxes under both size sliders), `buzz-app/src/editor.rs` (`Editor::tool_style` field + `refresh_tool_style`, called at the top of `pointer_down_at`/`pointer_move_at`/`pointer_up_at`; every `ToolContext` built from `&self.tool_style`), `buzz-app/src/app.rs` (cursor ring: fixed screen radius when a tool's flag is off); tests: `eraser_size_holds_its_screen_size_when_zoom_size_is_off`, `eraser_size_grows_with_zoom_when_zoom_size_is_on`, `brush_size_holds_its_screen_size_when_zoom_size_is_off`, `brush_size_grows_with_zoom_when_zoom_size_is_on` |
| H2 | Highlight layers (Animate's Highlight Layer) | Impl H | DONE — `crates/buzz-scene/src/layer.rs` (`Layer::highlight: Option<Color>`), `crates/buzz-doc/src/serial.rs` (`LayerDto::highlight`, format version 44), `crates/buzz-ui/src/panels.rs` (`HIGHLIGHT_SWATCHES`, `highlight_menu_body`, Layers panel row bar+wash and context menu), `crates/buzz-ui/src/timeline_panel.rs` (`TimelineResponse::set_highlight`, name-cell + frame-row bar/wash, `layer_context_menu` Highlight/Highlight Selected Layers submenus), `crates/buzz-app/src/app.rs` (`apply_timeline` → `doc.edit("Highlight Layer", ...)`); tests: `buzz-doc::serial::tests::a_layers_highlight_round_trips`, `buzz-app::app::idle_tests::a_layers_highlight_is_set_and_cleared_through_the_response_path`; `cargo test -p buzz-scene --lib layer` 52 pass, `cargo test -p buzz-doc --lib` 212 pass, `cargo test -p buzz-ui --lib timeline` 39 pass, `cargo build -p buzz-app` pass (one retry needed — a concurrent agent's text-styling edit briefly broke `editor.rs`, unrelated to this change) |
| N1 | New document: first layer spans one minute (fps × 60 frames) | Opus | DONE — `Editor::create_document` + `new_document_frames` (editor.rs); covers startup and File ▸ New; test `creating_a_document_uses_the_settings_asked_for` (1500 frames @25fps, doc clean) |
| J1 | BuzzAnimate equivalents of 6 Animate JSFL commands (autoTalkingAnimation, eyebrowsUp, eyeDown, copyExtendFrames, claasicTweenOnAllLayers, extendFrames) — user: **no tests**, do **after all other items** | Opus/Impl | PARKED — implementer stopped by user 2026-09-27 mid-run (partial buzz-script/prelude changes may be on disk, unverified); user: ignore for now |
| R1 | Opus review: targeted tests (user: no snapshot renders) | Opus | TODO |

## Root causes found (Opus)

- **M1**: `timeline_panel.rs` `layer_row` → `response.context_menu(|ui| { clicked_frame(ui) ... })`
  re-reads `pointer.interact_pos()` *every frame the menu is open*, so moving
  toward the popup re-selects cells and re-scrubs. Same pattern: `in_name_column`
  (computed from the live pointer each frame), `light_row`'s `menu_frame`,
  `ruler`'s context menu.
- **L1/L2**: `lights::gizmos(scene)` reads the *base* light, but an animated
  light renders from its track (`LightTrack::state_at`), so the handle sits at
  the base position, not where the light is at this frame. A drag edits the base,
  which the track overrides, so nothing moves until release
  (`editor.rs pointer_up_at` keys only then, and keys base values for every other
  field — clobbering interpolated intensity/colour at that frame).
- **T0**: egui `Visuals::strong_text_color()` = `widgets.active.fg_stroke` was
  `accent_text` (black for Dark's `#378ef0`); 59 `.strong()` sites.

## Specs

### Impl A — lights (files: `crates/buzz-light/src/{lib,track}.rs`, `crates/buzz-app/src/lights.rs`, `stage.rs` gizmo loop, `editor.rs` light fns + light gesture, `app.rs` `fn light_panel`)

- Add `LightRig::keyed_at(frame) -> Cow<LightRig>`: applies each light's track
  (`state_at`) only — **no** flicker/storm (those are render effects, not the
  light's editable state).
- `lights::gizmos_at(scene, frame)` / `target_at_frame(...)` built on
  `keyed_at`; keep `gizmos(scene)`/`target_at(...)` as frame-0-free wrappers only
  if tests need them. Stage (`stage.rs` ~1121) and `begin_light_gesture` use the
  current frame.
- Light drag (`pointer_move_at` and `pointer_up_at`): helper
  `fn drag_light_at(scene, gesture, point, frame, auto_key: bool)`:
  1. if the light's track animates, copy keyed state at `frame` into the base
     animatable fields first (base is inert while the track animates);
  2. `lights::drag`;
  3. if track animates → `set_key(LightKey::from_light(frame, light))`;
     else if `auto_key` → create track: when `frame > 0` key frame 0 with the
     **pre-drag** state, then key `frame` with the new state; enable track.
  Called on every move (live feedback) and on release; still one undo step
  (`end_gesture` on release only, as now).
- Light panel: show `keyed_at(current_frame)` values; on `response.changed` for
  a light whose track animates (or auto-key on), write the key at the playhead
  instead of only the base (same helper logic, minus the drag).
- Tests (in `lights.rs`/`editor.rs` test modules): gizmo of a keyed lamp sits
  at the interpolated position mid-span; dragging an animated lamp at frame 10
  moves it at frame 10 during the drag and leaves frame-0 key untouched; drag
  with auto-key on an un-keyed lamp at frame 12 yields keys {0: old, 12: new};
  keying preserves interpolated intensity at that frame.

### Impl B — timeline (files: `crates/buzz-ui/src/timeline_panel.rs`, `crates/buzz-scene/src/{layer,camera_track}.rs`, `crates/buzz-doc/src/serial.rs` camera section, `app.rs` `apply_timeline`, `editor.rs` camera fns)

- **M1**: capture on `response.secondary_clicked()` (the frame the menu opens):
  store `(frame: Option<u32>, in_name_column: bool)` in `ui.ctx().data` under an
  id keyed by the row (layer id / light id / "ruler"); read it inside the menu
  closure. Selection + scrub happen once, at open, not every frame.
- **P1**: in `timeline_panel`, inside the grid scroll area: if
  `state.current_frame` differs from last drawn (temp data) or `state.playing`,
  and the playhead's x is outside `[pinned_left + LAYER_COLUMN, clip.max.x -
  cell]`, `ui.scroll_to_rect` so the playhead lands near the grid's left edge
  (page like Animate). Never while the user is dragging the grid/ruler. Must not
  fight a user's manual horizontal scroll when the frame hasn't changed.
- **H1**: ruler's pinned heading: paint `panels::paint_layer_icon` per column
  (on = every layer has it on) instead of the 8 pt words; hover tooltip
  "Show/Hide All Layers" / "Lock/Unlock All Layers" / "Outline All Layers".
  Existing `out.toggle_all` path unchanged.
- **C1**: `CameraTrack.locked: bool` (serde default false; serial round-trip +
  test). Camera row: eye + lock switches in the same `switch_columns` positions.
  Eye = camera on/off (`ToggleCamera`); `shows_camera_row` becomes
  `edit_path().is_empty() && (enabled || !camera.is_empty())`, drawn dimmed when
  off. Lock refuses `MoveCamera`, camera key add/remove, camera moves with a
  status message. New response fields `toggle_camera: bool`,
  `toggle_camera_lock: bool`. Layer-tools strip: a camera button
  (selected when enabled) raising `Command::ToggleCamera`, tooltip
  "Add/Remove Camera".
- **F1**: rename the "Fld" button text to "Folder" (tooltip unchanged).
  Scene op `LayerStack::move_to(id, index, parent: Option<LayerId>) -> bool`:
  moves the layer **and all descendants** as one contiguous block, sets `parent`;
  refuses a folder into itself or its descendant; parent must be a Folder or
  None. Timeline drop zones on a row: top 25 % = above (parent = row's parent),
  bottom 25 % = below (parent = row's parent; below a Folder = first child of
  it), middle 50 % of a **Folder** row = into it (highlight the whole row); on a
  non-folder middle = nearest of above/below. New response field
  `move_layer: Option<(LayerId, usize, Option<LayerId>)>` replacing use of
  `reorder_layer` for timeline drags; app applies via one undoable edit.
- **F2**: `toggle_layer`/context-menu switch on a Folder applies the new value
  to the folder and every descendant (one undo). Layer context menu:
  "Move Out of Folder" when `parent.is_some()` (parent = folder's parent,
  placed just after the folder's block).
- Tests: context-menu frame stays fixed as the pointer moves (egui test harness
  as in `a_secondary_click_opens_the_context_menu_on_a_draggable_widget`);
  `move_to` block moves/cycle refusal/out-of-folder; drop-zone decision as a
  pure fn; camera row visible when disabled-but-keyed; locked camera refuses a
  nudge; serial round-trip of `locked`; playhead-follow as a pure fn deciding
  the target scroll x.

### Impl C — text (files: `crates/buzz-scene/src/object.rs` TextData, `crates/buzz-text/src/lib.rs`, `crates/buzz-doc/src/serial.rs` TextDataDto, `editor.rs` text fns + `drawing_group_of`, `app.rs` Properties text editor ~3570)

Added mid-session (user, 2026-09-27): "if two texts that I added separately
touch they shall be selected separately and move; I shall be able to select
one or a few characters of the text and change their style."

- **X1 root cause**: text is stored as a plain `ObjectKind::Shape` with
  `Object::text = Some(..)`, and `Editor::drawing_group_of` (editor.rs ~2958,
  last commit's whole-drawing pick) floods across every *touching* raw shape —
  so two touching texts become one drawing. Fix: an object with `text.is_some()`
  is never a seed of a flood (returns `vec![id]`) and never swept into another
  drawing's group. Test: two overlapping texts, click one → only it selected
  and only it moves.
- **X2 model**: `TextData.runs: Vec<TextRun>` — `TextRun { start: usize, end:
  usize` (char indices, half-open, sorted, non-overlapping, only where differing
  from the base) `, bold: Option<bool>, italic: Option<bool>, size:
  Option<f64>, color: Option<Color>, font: Option<String> }`. Empty = today's
  behaviour. Serialized in `TextDataDto` with `#[serde(default,
  skip_serializing_if = "Vec::is_empty")]`; bump the format version the way
  v37 did; round-trip test; old files load unchanged.
- **X2 layout**: `buzz_text::outline_runs(content, base size/font/style, align,
  spans) -> Vec<(BezPath, span index)>` laying spans sequentially on a shared
  baseline per line (advance = measured width of each span in its own
  face/size; line height = max size on the line). Single uniform span must
  produce the same outline as `outline_styled` (test).
- **X2 representation**: no renderer change. Uniform text (no runs) stays an
  `ObjectKind::Shape` exactly as now. Text with runs becomes
  `ObjectKind::Group` of one filled Shape per contiguous (style, colour)
  segment, `Object::text` on the **group**; the group's transform is the text
  object's transform. `set_text_styled` (and a new
  `set_text_runs(id, TextData)`) rebuild the shape/children, converting
  Shape↔Group as runs appear/disappear, preserving id, transform, layer
  position, across frames (`update_object_across` as now). A text group must
  not open group-edit on double-click (treat like text); everything that
  checks "is text" must go by `o.text.is_some()`, not by kind.
- **X2 UI** (Properties text section in app.rs): use `egui::TextEdit::multiline(..).show(ui)`
  to read `cursor_range` (char range). When a non-empty range is selected,
  Bold / Italic toggles, size, font and a colour button apply to that range
  (write/merge runs); with no selection they apply to the whole text as today.
  Typing inside/around runs must keep run indices consistent (shift/clip runs
  by the edit's diff; a simple common-prefix/suffix diff is fine).
  Remember the last non-empty selection across frames (egui drops focus when a
  style button is clicked) in egui temp data keyed by object id.
- Tests: run merge/split helpers (apply style to a range; edit shifts runs);
  outline_runs uniform parity; Shape→Group conversion keeps id/transform;
  serial round-trip; X1 test above.

### Impl W — workspace (file: `crates/buzz-ui/src/workspace.rs` only)

User (2026-09-27), choice confirmed via question: "One tab stack" — a single
section whose tabs are Properties, Effects, Tool, Library, Assets, Cast (one
visible at a time), docked in `Dock::Right` where the inspector is today. Any
panel can still be dragged out / floated / docked elsewhere (existing
behaviour — must keep working).

- `PanelId::tab_title`: Properties → "Properties" (not "Props").
- `Workspace::animate()`: Library, Assets, Cast join `GROUP_INSPECTOR` in
  `Dock::Right` (home `Dock::Right`), ordered after ToolOptions; Properties
  stays front. Renumber `order` so the column order stays Layers, inspector
  stack, Color/Swatches, utility stack. `Dock::RightOuter` is then empty by
  default (AI's hidden home stays RightOuter). `essentials()`: its right group
  holds all six (Properties front), the rest hidden as today.
- Migration: new `VERSION_PROPERTIES_STACK` = next number, becomes
  `LAYOUT_VERSION`. A saved layout older than it in which Library, Assets and
  Cast are all still in `Dock::RightOuter` sharing one group, and Properties'
  group is still in `Dock::Right`, gets those three moved into Properties'
  group (appended after its existing tabs, not selected, not collapsed). Any
  other arrangement is left alone (the user customised it).
- Update/extend the workspace tests that pin the old default; add tests: default
  right section panels == [Properties, Filters, ToolOptions, Library, Assets,
  Cast]; migration moves an untouched old layout and leaves a customised one;
  essentials section contents.

### Impl Z — "Zoom size with stage" (files: `crates/buzz-ui/src/style.rs`, `crates/buzz-ui/src/panels.rs` tool options ~1464 eraser / ~1800 brush, `crates/buzz-app/src/editor.rs` ToolContext sites, `app.rs` cursor ring ~5610)

User (2026-09-27): "in brush and eraser both there shall be option of increase
size with zoom". Animate's Brush option "Zoom size with stage".

- `DrawStyle.brush_zoom_size: bool`, `DrawStyle.eraser_zoom_size: bool`,
  default **true** (today's behaviour: size is in document units, so it grows
  on screen as you zoom in). Off: the size is **screen pixels** — the mark's
  document size = `size / Editor::edit_scale_on_screen(pointer)`, so the ring
  and the mark stay the same on-screen size at any zoom.
- Tool Options: a checkbox "Zoom size with stage" under the Brush size slider
  and under the Eraser size slider, with a hover explaining both states.
- Editor: field `tool_style: DrawStyle` = `style` with `brush.size` /
  `eraser_size` converted when the flag is off, refreshed at the start of each
  pointer down/move/up (before the `ToolContext` is built); every
  `ToolContext { style: .. }` site uses `&self.tool_style`. Pencil unchanged.
- Cursor ring (app.rs): when the tool's flag is off, radius = size/2 px
  (unscaled); on, as now (`size * edit_scale_on_screen * 0.5`).
- Tests: flag off at view zoom 4 → an eraser stroke's `ToolAction::Erase.width`
  is `eraser_size / 4`; flag on → `eraser_size`; brush likewise.

### Impl H — layer highlight (after Impl B; files: `crates/buzz-scene/src/layer.rs`, `crates/buzz-doc/src/serial.rs` layer DTO, `crates/buzz-ui/src/timeline_panel.rs`, `crates/buzz-ui/src/panels.rs` Layers panel rows, `app.rs`/`editor.rs` response handling)

User (2026-09-27): "add functionality to highlight some layers to identify
easily down the track". Animate's Highlight Layer.

- `Layer.highlight: Option<Color>` (None default); serialized with serde
  default + skip-if-none; round-trip test; old files load unchanged.
- Layer context menu (timeline name menu and Layers panel menu): "Highlight"
  submenu with 6 swatches (yellow, orange, red, green, blue, purple — muted
  enough for both themes) + "Remove Highlight"; also "Highlight Selected
  Layers" applying to every selected layer. One undo step, label
  "Highlight Layer".
- Timeline: highlighted layer's name cell gets a solid 4 px bar in the colour
  at the left edge plus a ~22 % alpha wash of the colour over the name cell and
  across the whole frame row (under the cells' marks, over the background). The
  selected-layer blue still wins on the name cell, but the left bar stays
  visible. Layers panel row: same bar + wash.
- New response field / command path so the app applies it through
  `doc.edit`; tests: set/clear highlight via the response path; serial
  round-trip.

### Impl J — Animate command ports (LAST; user: no tests)

Sources (read by Opus 2026-09-27): `%LOCALAPPDATA%\Adobe\Animate 2023\en_US\Configuration\Commands\`
`autoTalkingAnimation.jsfl` (1056 lines), `eyebrowsUp.jsfl`, `eyeDown.jsfl`,
`copyExtendFrames.jsfl`, `claasicTweenOnAllLayers.jsfl`, `extendFrames.jsfl`, all
reading `commonVariables.jsfl` (`CONFIG`: moveFrames 6, moveAmount 5,
moveAmountlESS 3, browFrames 10, browMoveAmount 5, browDelayFrames 12,
limbMoveFrames 10, handSwayDegrees 5, wristSwayDegrees 90, palmSwayDegrees 90,
bodySwayDegrees 5, …).

What they do:
- **extendFrames**: prompt seconds (default 185) → every non-folder layer
  extended (insert frames after its last) or trimmed (remove frames) to
  `round(seconds × fps)`.
- **copyExtendFrames**: for each selected frame range (layer, start, end) ask
  loop count (suggest filling to 185 s × 24); copy the range, insert
  `count × span` frames after it, paste the copy `count` times, clear
  keyframes inside the looped region, trim the layer to 185 × 24 frames.
- **eyebrowsUp**: from the playhead, on layers `leftEyeBrow` and `rightEyeBrow`:
  keyframes at start and start + browFrames, end element `y −= browMoveAmount`,
  classic tween; after browDelayFrames, the same back down.
- **eyeDown**: layer `eyeBall`: keyframes at playhead and + moveFrames, end
  element `y += moveAmountlESS`, classic tween.
- **claasicTweenOnAllLayers**: toggle. If a layer has any classic tween,
  remove tweens on all its frames; otherwise set a classic tween across all its
  frames; skip folders.
- **autoTalkingAnimation**: needs one selected character instance (or run from
  inside it); finds the sound layer (on this or the root timeline) → range
  = its duration from the playhead (prompt to confirm the end frame); enters the
  character symbol (or a container's nested character); extends its timeline;
  sets nested symbols' firstFrame to the playhead; renames layers to canonical
  names (rightArm/Wrist/Palm, leftArm/Wrist/Palm, body; inside Head:
  left/rightEyeBrow, eyeBall, mouth, eyeWhite); clears keyframes after the
  playhead on those layers, keys start and end; random hand/body gestures
  (rotate about the transformation point, out-gap-back with classic tweens,
  mirrored-matrix aware); enters Head: random brow gestures (up/down translate,
  rotate) and eye glances (8 directions); leaves the mouth layer and instance
  selected for lip-sync.

Plan: (J1a) extend `buzz-script` host + `prelude.js` with the JSFL subset these
use:
- `timeline.layers[i].frames[f]` → `{isKeyframe, startFrame, duration,
  elements[], tweenType (get/set "none"|"classic"), soundLibraryItem
  {name, sourceFilePath}}`; `layer.layerType` ("normal"|"folder"|…),
  `layer.parentFolder`, `layer.frameCount`;
- `timeline.setSelectedLayers(i | [i])`, `setSelectedFrames(start, end)` and
  the `[layer, start, end, …]` array form, `getSelectedFrames()`,
  `insertKeyframe(frame)`, `insertFrames(n, allLayers, atFrame)`,
  `removeFrames()`, `clearKeyframes()`, `copyFrames()`, `pasteFrames()`,
  `createMotionTween()` (a classic tween on the selected span);
- element `{elementType, symbolType, libraryItem {name, itemType, timeline},
  x, y (get/set), matrix {a, b, c, d, tx, ty} (get/set),
  getTransformationPoint(), firstFrame (get/set)}`;
- `document.selection` returns real elements; `document.enterEditMode('inPlace')`
  / `exitEditMode()` (symbol timelines); `doc.timelines[0]`;
  `fl.runMenuCommand("Insert Classic Tween" | "Clear Classic Tween" |
  "Remove Tween")`; `fl.configURI` + `fl.runScript` resolving to the
  BuzzAnimate commands folder; `fl.showIdleMessage` no-op; `FLfile.exists`.
- **Interactive `prompt`**: running a saved command that asked questions shows
  the questions with the script's defaults in a small dialog and re-runs with the
  user's answers (the first run is discarded, not committed). Unattended runs
  keep today's default-answer behaviour.
(J1b) write the six commands into `commands_root()` (and copy them to
`docs/commands/`), plus `commonVariables.js`, keeping the originals' structure
and names so they read like the Animate versions. No tests (user's instruction).

## Review log

(append after each review)

- 2026-09-27 Opus, L1–L4: reviewed `LightRig::keyed_at`, `lights::{gizmos_at,target_at_frame,drag_light_at,apply_light_edit,key_after_edit}`, stage/editor/panel call sites. Logic matches spec (base synced from track before drag; auto-key seeds frame 0 with pre-edit state). Implementer ran buzz-light 86 pass, buzz-app lights 29 pass. Accepted.
- 2026-09-27 Opus, W1: reviewed `untouched_assets_outer_group` + `VERSION_PROPERTIES_STACK`; migrates only when Properties is still in Dock::Right and Library/Assets/Cast all still share one RightOuter group. Implementer: `cargo test -p buzz-ui --lib` 348 pass. Accepted. (Snapshots skipped at user's request.)
- 2026-09-27 Opus, T0/E1: done by Opus, tests listed in Status.
- 2026-09-27 Opus, Z1: reviewed `refresh_tool_style` / `tool_style` (all ToolContext sites) + cursor ring; implementer tests zoom_size×4, panels, eraser pass; re-ran `-- creating_a_document zoom_size document` 21 pass. Accepted.
- 2026-09-27 Opus, W1 RE-OPENED: `cargo test -p buzz-app --lib app::` 57 pass / 8 fail — 2 real (far-right column overruns a 1920 window by 24 pt; a column 1 pt off the right) + 6 dock_drag tests pinned to the old Library section. W1's 'nothing pinned outside workspace.rs' claim was wrong. Fresh implementer on it.
- 2026-09-27 Opus: Impl B stopped by the user mid-run. Code for M1/P1/H1/C1/F1/F2 is present (secondary_clicked capture, scroll_to_rect, header icons, camera toggle/lock, LayerStack::move_to, Move Out of Folder) and builds; no tests written or run; status table was never updated by it. Not resumed pending user direction.
- 2026-09-27 W1 follow-up: root cause was two-fold — `section_header`'s Timeline row let its content grow past the column width egui gave it, so a Panel's *reported* rect (not just the stage) slid past the window edge; and the geometry test's own "draw the three columns as `docks` does" harness put a zero-width `Panel::right` on screen (RightOuter now empty by default) whose placeholder label overflowed the same way. Fix: `build_ui` now clamps every dock rect it stores to the window rect (`.intersect(window_rect)`) before pushing to `dock_rects`, so a swollen response never gets reported past the window; the geometry test's workspace now docks the AI panel into `RightOuter` (a real, occupied column) instead of leaving it empty, so the three-column drag math it exercises stays meaningful. Tests updated: `dock_geometry_tests::the_columns_never_overlap_however_wide_they_are_dragged` (setup only), `dock_drag_tests::{group_with_menu_offers_the_library, proximity_border_highlight_when_close_to_section, dropping_on_a_sections_lower_edge_docks_a_row_below_it, dropping_a_section_between_two_others_docks_it_there, grip_dragging_the_inspector_onto_the_library_tabs_it_there, grip_dragging_the_inspector_onto_the_stage_floats_one_window}` retargeted from Library (now a tab, not a section front) to Color/Swatches, plus `Drive`'s screen made taller so Color's section stays on screen below the six-tab inspector. `cargo test -p buzz-app --lib app::` 65 pass; `cargo test -p buzz-ui --lib workspace` 73 pass.
- 2026-09-27 Opus, W1 follow-up accepted: root cause a bottom-dock header row swelling past its column (egui reports content rect); fix clamps recorded dock rects to the window (bookkeeping, not a visible change). Geometry test now docks AI in RightOuter since that column is empty by default; 6 dock_drag tests retargeted Library→Color/Swatches. Caveat: the header row's overgrowth itself is clipped, not removed.
- 2026-09-27 Opus, H2: accepted on implementer's tests (serial round-trip, response-path set/clear; buzz-doc 212, timeline 39 pass). Layer.highlight bumped format to v44 — CHECK after X2 lands that text styling did not also claim v44.
- 2026-09-27 Opus, X1+X2: accepted on implementer tests (buzz-text 15, buzz-scene 489, buzz-doc 212, app text 4). Format v45 (TextRunDto) sits after H2's v44 — no collision. Limitations: TextData has no base colour (unstyled segments take the object's existing fill); the Properties range-styling UI has no widget-level test.
- 2026-09-27 Opus: full `buzz-app --lib` 481 pass / 2 fail, and the 2 fail ALONE (`--test-threads=1`): `dragging_a_lamp_over_a_scene_stays_interactive` 101.6 ms/frame (limit 65 dev) and `dragging_a_selection_on_a_heavy_document_stays_interactive` 7.32 ms. Real regression somewhere in the stage rebuild — baselining on HEAD in a scratch worktree.
- 2026-09-27 Opus, perf follow-up: NOT a regression. Probe split: edit 0.01 ms, stage build ~17 ms (LAMP DRAG 17.8 ms vs 65 dev limit). Both `stays_interactive` tests pass on two clean `--test-threads=1` runs; HEAD worktree also passes. Earlier failures coincided with another agent's background test run (CPU contention).
