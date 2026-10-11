# Editor, lighting and camera implementation plan

Date: 2026-09-21. Status: source audit and planning complete; implementation and runtime verification pending.

Progress record: [EDITOR_LIGHTING_PROGRESS.md](EDITOR_LIGHTING_PROGRESS.md).

## Scope and evidence

This plan covers editable wall shadows, animated shadow fidelity, tight selection bounds, predictable Free Transform, layer effects, vertical timeline operations, soft side lighting, and DSLR-style focus. It is based on the current working tree, including existing uncommitted changes. Preserve those changes and review their diff before implementation. Existing code is evidence of a capability, not proof that the user's reported bugs are fixed. No application or rendering tests were run for this planning pass.

| Request | Source findings | Work required |
| --- | --- | --- |
| Move, resize and tilt wall shadows | `buzz-light/src/lib.rs` has floor/wall targeting, shadow strength and length; `buzz-ui/src/light_panel.rs` exposes shadow settings | Add independent shadow placement and size controls with an on-stage gizmo |
| Shadows follow hands and bodies | `buzz-render/src/document.rs` has a separate `cast_shadows_within` traversal with posed rigs, warps and symbol frame resolution | Reproduce mismatches and align shadow evaluation with visible animated geometry |
| Tight selection boundary | `buzz-app/src/editor.rs::selection_bounds_drawn` derives bounds from projected object quads; `stage.rs` draws the box | Audit bounds at every coordinate conversion and distinguish artwork bounds from effect/culling bounds |
| Predictable transform handles | `tools.rs::transform_zone` already distinguishes corner, rotation ring, side midpoint, edge and pivot | Fix hit-zone conflicts, cursor/gesture disagreement and coordinate-space drift |
| Layer effects | `buzz-scene/src/layer.rs` stores filters; `filter_panel.rs` offers Object/Layer targets, brightness, blur and drop shadow; its blend selector is object-only | Make layer controls accessible, add layer alpha/tint and whole-layer Overlay blending, verify compositing |
| Vertical frame commands | `selection.rs` stores selected layers; `editor.rs::frame_op` and `set_tween` still target the active layer | Introduce explicit frame-cell selection and route all command entry points through batch operations |
| Soft side lighting | Existing lighting uses vector shading and silhouettes, not actual surface normals | Fix direction/softness on a complete character; provide controllable 2.5D surface approximation |
| Focal length / focus blur | `camera_track.rs` contains `focal_distance`, aperture, focus depth and focus keys; `panels.rs` exposes DOF; `camera_panel.rs` labels focal distance “Camera depth” | Clarify existing controls, add focus picking and an optional lens model |

Paths above are relative to `crates/`. Relevant integration points also include `buzz-app/src/app.rs`, `buzz-doc/src/serial.rs`, `buzz-light/src/track.rs`, `buzz-render/src/lighting.rs`, `buzz-render/src/compositor.rs`, `buzz-fx`, and `buzz-export`.

## Delivery order

1. P0: reproduce failures and establish reference scenes.
2. P1: fix selection bounds and transform interaction together.
3. P2: finish vertical timeline selection and batch commands.
4. P3: unify evaluated artwork and shadow geometry.
5. P4: add editable shadow placement.
6. P5: complete layer effects and isolated layer compositing.
7. P6: improve side lighting.
8. P7: improve camera focus and depth blur.
9. P8: compatibility, performance, export and user documentation verification.

P4 depends on P3 so manual placement does not conceal pose errors. P6 reuses P3 silhouettes. P7 reuses P5's isolated render surfaces where necessary. P1 and P2 can ship before rendering work. Each phase ends with its acceptance checks, and the tracker records evidence before marking it complete.

## P0 — Reproduction and baseline

- Review existing edits in editor, tools, stage, timeline, selection and lights; preserve their intended behavior.
- Build small deterministic scenes: a waving rig and nested animated symbol against a wall; one face with left/right lamps; rotated/skewed objects and transparent bitmaps; overlapping artwork with layer effects; foreground/subject/background at distinct depths; a timeline with locked, hidden, folder and normal layers.
- Capture exact frame, zoom, camera state, light settings, expected result and actual result for each report. Include scrubbing backward and export as well as forward playback.
- Compare existing tests before adding regressions. Inspect `buzz-app/tests/stage_lighting.rs`, `light_keyframes.rs`, `lighting_settles.rs`, `buzz-ui/tests/timeline_virtualization.rs`, `buzz-export/tests/headless_lighting.rs`, `headless_keyed_lights.rs` and `buzz-render/tests/compositor.rs`.
- Record baseline render times and UI response on these fixtures; do not infer performance from comments or old documentation.

Acceptance: every reported problem has either a reproducible failing case or a documented runtime result establishing what remains unresolved.

## P1 — Selection bounds and Free Transform

### Geometry contract

- Define tight artwork bounds independently from broad-phase hit-test bounds, filter padding, shadow extents and render culling bounds. Blur, drop shadows, lamp reach and rotation hit rings must not inflate the selection rectangle.
- Evaluate paths, stroke caps/joins, groups, symbol frames, rigs and warps at the displayed time. For bitmaps, use cached nontransparent content bounds with a documented alpha threshold; invalidate after painting/import changes.
- Derive bounds from transformed evaluated geometry instead of repeatedly transforming axis-aligned bounding boxes. Keep coordinate conversions explicit across object, symbol-edit, layer-parent, depth, camera and screen spaces.
- Show an oriented box for a single transformed object where supported. Use a tight union for multiple objects. A rectangular box encloses the actual artwork; it need not trace an irregular silhouette.
- Share the same projected handle geometry between drawing, hover cursors and pointer hit-testing. Keep visual padding small and hit tolerance constant in screen pixels across zoom and DPI.

### Interaction contract

| Pointer location | Hover feedback | Drag behavior |
| --- | --- | --- |
| Corner square | Resize cursor and “Resize” hint | Scale about opposite corner; Shift preserves aspect ratio; Alt uses pivot |
| Just outside corner | Curved rotation cursor and “Rotate” hint | Rotate about pivot; no move fallback |
| Bottom/top midpoint square | Vertical resize cursor | Compress or elongate height; opposite edge stays fixed unless Alt is held |
| Left/right midpoint square | Horizontal resize cursor | Compress or elongate width |
| Edge line between midpoint and corner handles | Skew cursor and hint | Shear along that edge's axis |
| Pivot marker | Pivot cursor | Move pivot only |
| Interior artwork away from handles | Move cursor | Translate artwork |
| Exterior beyond transform zones | Neutral cursor | No accidental translation |

- Make both corner actions discoverable by showing resize and rotate markers on hover. Dedicated handles win over edge skew; resolve tiny-object overlaps deterministically and expose accessible numeric controls when handles cannot fit.
- Capture operation, initial matrix, pivot and anchor on pointer-down. Evaluate each update from that snapshot, rather than accumulating deltas or switching operations mid-drag.
- Preserve the anchor during scale/skew and the pivot during rotation. Object coordinates can change as a consequence of those transforms; there must be no unintended translation gesture.
- Escape restores the snapshot; release creates one undo entry; a click without a drag changes no geometry. Guard zero-size bounds, near-singular transforms, negative scale and crossing the pivot.

Acceptance: corners resize/rotate without jumping, bottom midpoint changes only height, edge segments skew, pivot moves independently, and visible handles match their hit zones at multiple zooms/DPI values, inside symbols, with parenting and a tilted camera. A single undo restores each gesture.

## P2 — Vertical timeline selection and batch commands

- Represent timeline selection explicitly as stable layer IDs plus an inclusive frame range and an anchor cell. Keep it separate from artwork selection and the active layer.
- Vertical dragging selects the same frame across rows; rectangular dragging extends both axes. Shift extends from the anchor. Add “Select frame column across all layers” to the timeline menu; preserve selection during scrolling and virtualization.
- Right-click inside a selected region preserves it. Right-click outside starts a new selection at that cell. Commands must not silently collapse selection to the active layer.
- Route context menu, toolbar, command palette and keyboard through one target-resolution function and one undo transaction.
- F6 inserts keyframes and F7 inserts blank keyframes in selected cells. For a range, operate at each selected frame; show the range and resulting scope clearly. Deduplicate targets and preserve unrelated later keys.
- Classic Tween applies to distinct eligible spans touched by the selection, per layer. Do not invent endpoint poses: report spans without a valid following key or with incompatible content. Report changed and skipped counts with reasons.
- Retain hidden-layer selection for explicit timeline edits; skip locked/effectively locked layers and unsupported row types. Folder rows expand to eligible descendants for the all-layers action; deduplicate children. No-op batches create no undo entry.
- Keep F6/F7. Add a configurable default shortcut for Classic Tween, provisionally Ctrl+Alt+T after checking the shortcut registry for collisions. Display the actual configured binding in menus. Scope shortcuts to timeline/editor focus, excluding text inputs.

Acceptance: selecting one column across ten editable layers and using either menu or shortcut creates ten keys/blanks in one undo step. Classic Tween processes eligible spans across all selected layers, reports invalid spans, and preserves unrelated content. Locked layers remain intact; undo/redo and save/reopen preserve results.

## P3 — Animated shadow fidelity

- Trace visible-object evaluation versus `cast_shadows_within`. The shadow walker already poses armatures and warps, but nested symbols walk `objects_at(inner)`; compare this with tweened, modified and parented visible rendering. Treat discrepancies as candidates until reproduced.
- Build a shared evaluated geometry/silhouette path using the same sample time, nested symbol clock, tween resolution, IK/warp state, procedural modifiers, parenting, visibility and projection as visible artwork.
- Apply masks and alpha consistently to silhouette coverage. The current shadow traversal explicitly skips mask geometry without clipping its children; fix that gap. Define partial-opacity casting and avoid repeated darkening where parts overlap.
- Keep filter-generated decorative drop shadows separate from light-cast shadows so they do not recursively become casters.
- Key caches by evaluated geometry/deformation revision and sample time where needed. Async cache results must not replace newer poses. Playback, scrubbing, onion skins and export must not reuse the wrong pose.
- Share evaluation at fractional frame times with export motion blur; avoid a separate integer-only shadow animation path.

Acceptance: raised hands, body turns, nested symbol tweens, puppet warp, live modifiers, masks and hidden parts cast matching silhouettes on every sampled frame. Backward scrubbing and exported frames agree with the stage. Overlapping body parts form one consistent shadow tone.

## P4 — Shadow position, size and tilt

- Add a named Shadow section to light properties, retaining automatic light-based projection as the default.
- Provide offset X/Y, width/height scale, linked scale, rotation, horizontal/vertical skew, pivot, opacity, softness and Reset. Interpret “tilt” as planar rotation/skew initially, and label these explicitly.
- Store identity-default adjustments per light, with optional per-caster overrides for independent character shadows. Use stable light/caster IDs; clean references on deletion and remap on duplication.
- Compose controls in receiver-local space after physical projection: final point = receiver-to-scene × manual adjustment about pivot × projected caster point. Moving the lamp still updates projection; manual adjustments remain editable offsets.
- Add an explicit Edit Shadow mode: select a cast shadow and drag its own translation/resize/rotate/skew gizmo without moving the character or light. Numeric controls must match gizmo behavior and support reset.
- For an explicit wall receiver, store its reference and clip shadows to its visible surface. Retain a compatible implicit wall/floor mode for existing scenes. Handle deleted receivers predictably.
- Extend serialization, copy/paste/duplication and light tracks for shadow parameters. Interpolate offsets/scales continuously and rotations by a defined shortest-path rule; switching receivers is discrete.
- Recompute conservative render bounds after manual transforms and softness so shadows are not clipped or culled prematurely. Handle extreme low-angle lights without unbounded allocations.

Acceptance: a wall shadow can be independently translated, widened, shortened, rotated and skewed, then reset to automatic placement. Animation still changes its silhouette, lamp movement still changes projection, and values survive undo/redo, keyframing, save/reopen and export.

## P5 — Layer brightness, alpha, tint, blur, drop shadow and Overlay

- Expose an obvious Layer Effects section whenever a layer is selected, without requiring artwork selection or conversion to a symbol. Reuse existing filter models and controls where correct.
- Provide brightness, opacity/alpha (0–100%), tint color and amount, blur X/Y, drop shadow color/opacity/angle/distance/softness, and layer blend mode including Overlay. Also offer Color Overlay as a solid-color effect so the two meanings of “overlay” are unambiguous.
- Add neutral-default layer color/opacity/blend fields only where existing models cannot represent the behavior. Route all controls through undoable commands with gesture coalescing.
- Composite the layer's artwork and masks into an isolated transparent surface when whole-layer semantics require it. Applying 50% opacity separately to overlapping shapes must not darken their overlap differently from the rest of the layer.
- Define order: resolve artwork/object effects and masks, apply layer color adjustments, apply the ordered layer filter stack, apply camera DOF, then layer opacity/blend into the scene. Evaluate physical cast shadows separately and specify their layer-opacity policy; decorative drop shadow stays in the effect stack.
- Use premultiplied alpha correctly. Define and test the color space for brightness/tint/Overlay. Add sufficient filter padding, avoid edge halos, and preserve transparent export.
- Keep reorder, enable/disable, remove and reset controls. Supply a cheap neutral-effects fast path and bounded surface/cache sizes.

Acceptance: all requested controls visibly affect a whole layer, are reachable without selecting an object, match stage/export, and survive serialization. Overlap, masks, transparent pixels and reordered effects have predictable results. Overlay is checked against a small numerical reference.

## P6 — Soft directional lighting on faces and bodies

- Reproduce a lamp at the left, right and exactly beside a face, varying lamp depth and ambient fill. Verify direction uses evaluated figure coordinates rather than individual eye/mouth shapes.
- Use a coherent figure silhouette to construct a smooth light-to-dark transition. Expose ambient/fill strength, light intensity, light size/softness, and a surface-roundness control for the 2.5D approximation.
- Preserve facial details and base colors through shading. Multiple lights add contributions without repeatedly multiplying the character into darkness. Mirror the lighting when the lamp changes sides.
- Audit lamp falloff, elevation/depth and near-zero distances. Keep all calculations finite and clamp only at documented boundaries.
- Flat vector art has no actual face surface normals. Deliver controllable stylized volume first; a true normal-map/mesh material system is a separate extension, not an implied capability of these fixes.

Acceptance: a side lamp visibly brightens the facing side and leaves the far side darker with a soft transition; ambient fill controls darkness. Moving the lamp across the face reverses the result smoothly, including during hand/body animation and export.

## P7 — Camera focus and DSLR-style depth of field

### What already exists

`CameraTrack::focal_distance` controls projection/camera depth in document units. It is not a calibrated photographic focal length in millimeters. Existing DOF uses `aperture * abs(layer_depth - focus_depth)`, so it can soften layers on both sides of focus, but it is a simplified layer-depth approximation. Aperture, focus depth and keyed focus pulls already have controls in `buzz-ui/src/panels.rs`.

### Implementation

- Consolidate camera depth, focus depth, aperture/blur amount and focus-key controls in a discoverable Camera/Lens section. Explain units and retain existing controls for old files.
- Add Focus on Selected Object and a stage focus picker, resolving the object's effective depth. Add an optional animated focus target using stable IDs, with a fixed-depth fallback when a target disappears.
- Distinguish editor viewport zoom from render-camera lens zoom and dolly. Viewport zoom must not alter photographic focus. A camera zoom may retain focus on an enabled target; manual focus remains manual.
- Add an opt-in physical lens mode with focal length in mm, sensor size, f-number, focus distance and an explicit document-unit-to-distance scale. Validate near/far distances and prevent singularities.
- Derive blur radius from a thin-lens circle-of-confusion calculation, then convert sensor-space blur to output pixels. Define camera position and field-of-view mapping without silently reinterpreting legacy `focal_distance`.
- Preserve the legacy DOF model for existing documents. Objects sharing a depth plane necessarily share blur; expose layer/object depth authoring and document this limitation. Implement object-depth isolation if independent same-layer focus is required.
- Use depth-aware compositing for strong blur: background coverage must extend behind sharp foreground silhouettes, and foreground blur must not produce black/transparent fringes. A single final full-screen blur cannot satisfy this requirement.
- Keyframe lens/focus controls and evaluate fractional times consistently with export. Offer preview quality levels while final output uses the chosen render quality.

Acceptance: a distant subject remains sharp while nearer foreground and farther background blur; changing focus reverses which plane is sharp. Increased aperture opening increases blur. Focus pulls, camera motion and export remain consistent. Viewport zoom alone does not change the image's focus. Legacy scenes retain their appearance by default.

## P8 — Compatibility, validation and completion

- Inspect the current document format version at implementation time; allocate migrations for newly persisted fields with identity defaults. Avoid relying on old roadmap version numbers.
- Add round-trip and legacy-file tests for shadow transforms, receiver references, layer effects and lens mode. Check undo/redo and duplicate/delete remapping.
- Run targeted tests per phase, then `cargo test --workspace` and `cargo clippy --workspace --all-targets` for the integrated change. Record failures, skipped GPU tests and environmental limits honestly.
- Use existing headless GPU fixtures for image comparisons at identical time/camera/output dimensions; add numerical assertions for anchors, transforms, blur radii and compositing. Manual checks cover hover discoverability and gesture comfort.
- Compare playback/render cost against P0 at the same resolution and fixture. Keep UI input responsive, bound filter/DOF memory, cancel superseded preview work, and verify stable playback after caches warm.
- Validate PNG and an available video export path, including fractional samples when motion blur is enabled. Check that selection/light/shadow gizmos never appear in exports.
- Update user guides and shortcut documentation to describe actual shipped controls. Mark each tracker item complete only with implementation references and validation evidence.

Definition of done: every request has a passing acceptance scenario, persists where appropriate, works through undo/redo and export, and has evidence in the progress file. Source audit alone does not close implementation work.
