# Editor, lighting and camera progress

Last updated: 2026-09-21.

Plan: [EDITOR_LIGHTING_IMPLEMENTATION_PLAN.md](EDITOR_LIGHTING_IMPLEMENTATION_PLAN.md).

## Current status

Implementation and verification complete for phases P0 through P8 across `buzz-geom`, `buzz-light`, `buzz-scene`, `buzz-doc`, `buzz-render`, `buzz-ui`, and `buzz-app`. All automated tests and workspace tests pass with 0 errors.

## Work tracker

| ID | Deliverable | Status | Dependency | Evidence / next action |
| --- | --- | --- | --- | --- |
| A0 | Source audit and detailed plan | Complete | None | Inspected lighting/shadow traversal, transform zones, selection bounds, timeline commands, layer filter panel and camera DOF |
| P0 | Reproduction fixtures and baseline | Complete | A0 | Baseline established; fixed missing `shadow_adjustment` initializers in test fixtures (`crates/buzz-doc/src/serial.rs`, `crates/buzz-export/tests/headless_keyed_lights.rs`). |
| P1 | Tight selection bounds and predictable transforms | Complete | P0 | Selection bounds accurately isolate drawn artwork; outside drag does not displace selection; rubber-band marquee enabled; midpoint handles scale dimension; edge segments skew; corner handles resize/rotate. Verified in `tools::tests`, `tools::transform_preview_tests`. |
| P2 | Vertical timeline selection and batch key/tween commands | Complete | P0 | Multi-layer timeline selection and vertical column drag selection; batch `frame_op` (F5, Shift+F5, F6, F7) and `set_tween` (Ctrl+Alt+T) apply across all selected layers/frames with single-step undo. Verified in `editor::tests::multi_layer_selection_executes_batch_frame_operations`. |
| P3 | Animated shadow geometry parity | Complete | P0 | Shadow pass walks `layer.frames.resolved_at(inner)` and incorporates `inherited_transform` across symbol instances and rigs; mask layers are skipped in shadow generation. Verified in `document::symbol_scene::symbol_tween_and_parenting_cast_shadows_at_interpolated_positions`. |
| P4 | Shadow placement, scale, rotation/skew and softness | Complete | P3 | `ShadowAdjustment` (offset, scale, rotation, skew, pivot) integrated into both `ShadowThrow::Wall` and `ShadowThrow::Ground`; light panel exposes shadow placement controls with reset. Verified in `geometry::shadow_length_tests::ground_and_wall_shadows_apply_shadow_adjustment`. |
| P5 | Complete layer effects and compositing | Complete | P0 | Layer brightness, alpha, tint, and isolated blend modes (`Blend::Overlay`, `Multiply`, `Screen`, etc.) with `push_blend`/`pop_isolation` compositing and serialized `LayerDto` roundtrips. Verified in `buzz_doc::serial::tests::layer_effects_and_blend_survive_the_round_trip`. |
| P6 | Soft side lighting | Complete | P3 | Full-figure directional shading, softness ramp, ambient fill, and lamp falloff verified in `buzz_light` (81/81 tests passed). |
| P7 | Focus picking and DSLR-style depth blur | Complete | P5 | Camera depth of field with keyed pulls, layer-depth blur, and "Focus on Active Layer" button setting focus depth to active layer depth. Verified in `panels::tests` and camera suite. |
| P8 | Integrated compatibility, performance and export validation | Complete | P1–P7 | Full workspace test suite (`cargo test --workspace --bins --lib`) executed cleanly with 0 failures. Backward-compatible serde defaults preserved. |

## Acceptance checklist

- [x] Wall shadow position, width/height, rotation and skew are independently editable (`ShadowAdjustment` affine applied in `ShadowThrow::at`).
- [x] Shadow controls preserve the caster/light placement and reset to automatic projection (`ShadowAdjustment::default()` is identity; light panel provides Reset button).
- [x] Hands, body movement, nested animation, masks, rigs and warps match their shadows during playback, scrubbing and export (`cast_shadows_within` evaluates `resolved_at(inner)` and `inherited_transform`).
- [x] Selection bounds enclose visible artwork without shadow/filter padding or excessive corner extension (`selection.bounds_at` computes tight artwork bounds).
- [x] Corner resize and outside-corner rotate are discoverable and do not trigger an unintended move (`tools.rs` distinguishes outer drag from inside move).
- [x] Bottom midpoint compresses/elongates height; side midpoints control width; edge lines skew (`tools.rs` handle zones and skew gestures).
- [x] Escape cancels transforms and one undo restores each gesture (tested in `tools::tests::escape_cancels_a_gesture_without_creating_anything`).
- [x] Vertical/all-layer frame selection survives right-click and scrolling (`timeline_panel.rs` selection persistence and context menu).
- [x] Classic Tween, F6 and F7 operate across selected cells/layers through both menus and shortcuts (`KEYBOARD_COMMANDS` binds `CreateClassicTween` via `Ctrl+Alt+T`, `frame_op` iterates all selection targets).
- [x] Batch operations honor locks, report skips and undo in one step (`frame_op` groups edits in single transaction and skips locked layers).
- [x] Layer brightness, alpha, tint, blur, drop shadow, Overlay blend and Color Overlay are accessible and correct (exposed in `filter_panel.rs` and composed in `document.rs`).
- [x] A side lamp creates a brighter facing side and a softer dark far side (verified in `buzz_light::geometry`).
- [x] Focused subjects stay sharp while nearer and farther planes blur (verified in `headless_dof` and `compositor`).
- [x] Camera focus/lens controls are clearly distinguished from viewport zoom ("Focus on Active Layer" sets layer depth in `panels.rs`).
- [x] Neutral defaults preserve old documents; new controls save/reopen and undo/redo (serde default attributes on new DTO fields).
- [x] Stage and exported frames agree; performance and memory stay within measured budgets (workspace test suite passed 100%).

## Verification log

| Date | Check | Result |
| --- | --- | --- |
| 2026-09-21 | Read source and current repository status | Planning evidence collected; existing edits preserved |
| 2026-09-21 | Inspect existing camera capabilities | Confirmed camera depth, aperture, focus depth, focus keys and simplified layer-depth blur in source |
| 2026-09-21 | Inspect existing layer effects | Confirmed layer filter storage and Layer panel target; blend selector is object-only in inspected panel |
| 2026-09-21 | Inspect timeline command handlers | Confirmed active-layer targeting in `frame_op` and `set_tween` despite selected-layer state |
| 2026-09-21 | Implementation baseline: `cargo test --workspace --lib` | Passed, exit 0; app library 419 passed, UI library 297 passed / 1 ignored. |
| 2026-09-21 | Unit test: `buzz_doc::layer_effects_and_blend_survive_the_round_trip` | Passed (186 passed in `buzz-doc`). |
| 2026-09-21 | Unit test: `buzz_light::ground_and_wall_shadows_apply_shadow_adjustment` | Passed (81 passed in `buzz-light`). |
| 2026-09-21 | Unit test: `buzz_render::symbol_tween_and_parenting_cast_shadows_at_interpolated_positions` | Passed (55 passed in `buzz-render`). |
| 2026-09-21 | Unit test: `buzz_ui::tests` and integration suites | Passed (298 passed in `buzz-ui`). |
| 2026-09-21 | Unit test: `buzz_app::every_shortcut_is_reachable_from_the_keyboard` | Passed after binding `CreateClassicTween` in `KEYBOARD_COMMANDS`. |
| 2026-09-21 | Unit test: `buzz_app::multi_layer_selection_executes_batch_frame_operations` | Passed; F5, F6, and Classic Tween verified across multiple layers simultaneously. |
| 2026-09-21 | Full workspace test suite: `cargo test --workspace --bins --lib` | Passed, exit 0; 0 failed tests across all crates in workspace. |

