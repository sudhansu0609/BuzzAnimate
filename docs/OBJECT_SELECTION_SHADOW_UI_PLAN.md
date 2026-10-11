# Object Selection, Character Shadows, Right-Side UI, and Adobe Fidelity Plan

## Status

Planning only. Do not implement this plan until the user explicitly authorizes implementation. When implementation is approved, use GPT-5.6 Luna.

This revision incorporates Astra's review corrections. It is a plan, not a claim that the implementation or its validation is complete.

### Starting point and existing work

Earlier authorized implementation attempts left partial source changes before the user stopped implementation. Preserve those changes and all unrelated work. Do not assume a clean checkout or restart the work from scratch.

At the review, the following changes were present, but their presence alone does not establish acceptance:

| Area | Changes already present | Remaining verification or work |
| --- | --- | --- |
| Selection | Shared hover/drag classifier, default-pivot fallback, eight visible handles, and Subselection cursor changes | Include the drawn rotation hint in the shared geometry; verify display scaling, camera transforms, pivot overlap, and complete gesture behavior |
| Shadows | Changes to symbol timing, modifier evaluation, and parent transforms; an animated-symbol regression | Verify actual clipping and alpha coverage, warped artwork, nested timing, and all required regressions |
| Right-side UI | Partial inspector/palette grouping and layout migration | Complete the specified panel order and visual cleanup; verify custom-layout preservation and small-window usability |

Before any future implementation, inspect the current diff and establish a baseline for each area. Record what is verified complete, incomplete, or unverified, with the relevant files and test evidence. Earlier test-pass reports apply only to the versions and cases tested; they are not final sign-off.

## Goals

1. Make the Object Selection tool behave like Adobe Animate's transform interaction.
2. Make layer drop shadows follow the exact evaluated character pose shown on stage.
3. Replace the crowded right-side workspace with a compact, elegant, contextual inspector while retaining access to every existing option.
4. Import large Adobe Animate documents without losing assets, layer parenting, nested symbols, masks, alpha, gradients, or editable timeline structure; provide a verified editable return path to Adobe Animate.
5. Keep rotation handles attached to the selection and preserve the transformation point relative to the artwork throughout rotation, preview, commit, and subsequent gestures.

## 1. Object Selection and Transform Interaction

### Target behavior

The compatibility reference is Adobe Animate's **Free Transform** interaction, exposed through BuzzAnimate's Selection workflow as requested. This does not promise equivalence with every Adobe selection, distortion, or transformation mode. Use the Free Transform modifier rules rather than the different rules of Adobe's dedicated Scale mode. Reference: [Adobe's transformation documentation](https://helpx.adobe.com/animate/desktop/multimedia-and-video/transforming-combining-graphic-objects.html).

- Hovering directly over a corner shows a resize cursor.
- Hovering just outside a corner shows a rotate cursor/ring.
- Dragging a corner resizes the selection.
- Dragging just outside a corner rotates the selection around the transformation point.
- Hovering a middle edge shows the appropriate one-axis resize cursor.
- Hovering an edge away from its middle handle shows the skew cursor where skewing is supported.
- Dragging inside the selection moves the artwork.
- The transformation point can be moved and remains the pivot for rotation and constrained scaling.
- Shift-dragging a corner resizes proportionally; Shift-dragging the rotation zone snaps rotation in 45-degree increments.
- Alt-dragging the rotation zone rotates around the opposite corner on Windows; Option is the equivalent on macOS. Keep this distinct from any Alt/Option scaling gesture and test the two independently.
- A missing pivot defaults to the selection center until the user moves it.
- Behavior remains stable for small selections, zoomed views, camera panning, rotated artwork, nested symbols, groups, and selections spanning multiple objects.
- Subselection remains a point-editing mode and must not advertise transform gestures that it cannot perform.

### Implementation approach

1. Define one transform-zone classifier in the tool layer for corners, rotation ring, side handles, edge/skew zones, pivot, and inside selection.
2. Derive pointer-down classification, hover feedback, drawn handles, and rotation hints from the same handle geometry and interaction state. Do not leave a separate rotation-ring hit test in the drawing code.
3. Measure handle hit areas in screen-space logical pixels, consistently adjusted for display scaling. Project the geometry through the full camera and symbol placement before hit testing, then convert drag positions into edit space for transformation math. Dividing a radius by workspace zoom alone is insufficient when the document camera or symbol placement also scales the artwork. Handle non-invertible transforms explicitly without introducing invalid coordinates.
4. Resolve the effective pivot once per interaction, using the stored pivot or the selection center.
5. Resolve overlapping pivot/corner hit areas by proximity so a centered pivot does not steal a small selection's corners. Exception: if the user deliberately places the pivot exactly on a corner, the pivot wins at that point; the adjacent rotation zone remains available. Use the same priority in hover, drawing, and drag handling.
6. Draw all handles that are active for the current tool. Make the rotation affordance visible without obscuring the artwork.
7. Keep preview and commit transforms on the same calculation path so the visible drag preview exactly matches the undoable result.
8. Preserve undo grouping: one drag produces one history operation, and a cancelled/no-op drag produces no unwanted edit.

### Tests and acceptance checks

- Fresh Selection tool with no stored pivot: corner resize works.
- Corner vs. pivot hit priority on a very small selection.
- A deliberately moved pivot exactly on a corner remains draggable and still allows adjacent rotation.
- Rotation ring works at all four corners.
- Side handles resize the correct axis.
- Inside drag moves the object.
- Subselection still edits anchors and does not show transform cursors.
- Drawn handles and rotation hints, hover classification, and drag classification agree at the same pointer location.
- Zoom, pan, camera projection, rotated bounds, and edit-in-place selections use the same screen-to-edit conversion.
- Modifier keys affect preview and committed transform identically.
- Shift resizing, Shift rotation snapping, and Alt/Option opposite-corner rotation each have explicit tests.
- Handle reach stays consistent at 100%, 150%, and 200% display scaling, including combined document-camera zoom and symbol edit-in-place scaling.
- Undo/redo restores the original geometry and the final geometry.

## 2. Character Layer Drop Shadows

### Problem to eliminate

The shadow silhouette must represent the same pose, frame, nested symbol state, inherited transform, and live deformation as the character pixels drawn in that frame. A stale stored pose can make limbs, especially legs, appear in a different position from the character.

### Implementation approach

1. Identify the renderer's single authoritative artwork-evaluation path.
2. Reuse the same frame resolution for the shadow silhouette:
   - stage frame;
   - symbol-local frame;
   - root-synchronized instances;
   - graphic/movie clip timing;
   - layer keyframe elapsed time;
   - shape and classic tweens.
3. Traverse nested symbols with the same drawable-layer and mask rules as visible artwork. Apply the masks' actual clipping to the silhouette; skipping mask layers alone does not clip the artwork they mask.
4. Apply inherited layer transforms exactly once. Avoid applying parent placement both in the layer projection and in the silhouette transform.
5. Apply live modifiers to the same object evaluation used by the visible draw, including spring/wiggle/pose evaluation and breathing deformation.
6. Apply deformation after the same world/document transforms used by the visible drawing path. Include warped shapes through the same fill, stroke, and deformation handling as ordinary shapes.
7. Keep object filters and layer filters on their intended scopes while ensuring both use the evaluated silhouette.
8. Preserve culling, shadow reach margins, alpha, blur quality, and export/preview parity.
9. Derive shadow coverage from the evaluated subject's visible alpha, not merely the existence of a fill or an image's rectangular bounds. Include transparent bitmap regions, partial opacity, vector holes, and stroke widths/caps/joins. Select an alpha-mask or equivalent representation if a concatenated vector path cannot represent the required coverage; do not claim parity while omitting these cases.

### Tests and acceptance checks

- A character with two visibly different leg poses produces a shadow under the current pose at each frame.
- A real in-between tween frame is rendered; the test must reject either stored endpoint pose.
- A root-synchronized nested symbol uses the stage frame rather than an incorrect local frame.
- Nested inherited transforms are applied once and only once.
- Breathing and other live deformations affect the shadow in the same frame as the artwork.
- A mask actually clips the shadow silhouette to the visible subject; hidden portions do not cast a layer drop shadow.
- Transparent bitmap regions and vector holes remain empty in the shadow; partial opacity contributes the corresponding coverage.
- Stroke-only artwork and warped/deformed shapes cast shadows matching their visible outlines.
- Object-level and layer-level shadow/filter behavior remains correct.
- Preview and headless export render the same pose and shadow.
- Existing static shadow, glow, blur, blend, knockout, and symbol-instance tests continue to pass.

For the key regression, compare specific pixels or silhouette regions at the expected interpolated pose. A broad bounding-box assertion is insufficient because it can pass while the shadow remains at an endpoint pose.

Use a valid extended timeline and a genuinely changing pose. Render a nonzero in-between frame, verify the unfiltered artwork with a colour/alpha assertion that cannot also pass on the background, and compare its silhouette with a known-offset shadow. Reject shadows at both stored endpoint positions. Demonstrate that the regression fails with the old faulty behavior in an isolated copy or controlled reference path, and passes with the correction; preserve the working tree during this comparison. Add separate cases for nonzero placement keyframes, inherited root synchronization, parenting, and live deformation.

## 3. Right-Side Workspace and Panel Redesign

### Desired information architecture

The default main right column has this exact order, from top to bottom:

| Position | Group and tab order | Default state |
| --- | --- | --- |
| 1 | Inspector: Properties, Effects, Tool Options | Expanded; Properties selected; only the selected tab's body is shown |
| 2 | Layers | Expanded, with the layer-list body capped at 180 logical pixels and its own scrolling |
| 3 | Palette: Color, Swatches | Collapsed; Color selected when first expanded |
| 4 | Utilities: Story, Depth, Rigging, Lighting, Sound | Collapsed; Story selected when first expanded |

The outer right column contains Library, Assets, and Cast in that tab order, with Library selected and expanded by default. Layers retains visibility, lock, outline, parenting, and ordering controls. Every group remains available through the Window menu, and a user may rearrange the default.

In Properties, basic document/object settings start expanded; advanced camera and appearance sections start collapsed. In Effects, the target and effect list remain visible, the selected effect's parameters expand, and advanced options start collapsed. Tool Options displays the active tool's settings when chosen or explicitly revealed; a tool with no settings must not create a separate empty panel or automatically displace the user's selected tab.

### Visual and usability changes

1. Put the primary inspector first in the main right column, following the exact order above. Its tab strip must remain accessible while its body scrolls.
2. Select Properties by default for a new workspace. Reveal Effects when the user explicitly chooses an effect-editing command or the Effects tab; ordinary selection changes must not steal focus from another tab.
3. Keep Tool Options contextual: show the controls for the active tool, and avoid a large empty panel when the current tool has no options.
4. Use consistent panel padding, row height, label alignment, numeric field widths, section spacing, and heading treatment.
5. Use compact tabs and menus for secondary tools instead of persistent title bars for every panel.
6. Use collapsible sections for advanced controls such as depth of field, motion blur, colour grading, and filter parameters.
7. Replace repetitive full-size buttons with compact icon buttons or labelled menus where the action is discoverable through a tooltip.
8. Preserve clear labels, keyboard access, hover text, disabled states, and visible feedback for selected tabs and active controls.
9. Keep the Window menu capable of revealing every panel, including utility panels that are hidden from the default workspace.
10. Keep dock resizing, floating panels, tab selection, collapse/expand, and reset layout available.
11. Ensure narrow windows remain usable: reserve visible space for the inspector tab strip and Layers header, constrain expanded bodies, and scroll within those bodies. Keep each collapsed header reachable. At constrained widths, collapse the outer library dock to a labelled Library control that opens its panel; use overflow menus for tabs that cannot fit. Never squeeze labels and controls into overlapping rows.

### Workspace migration

- Update the shipped default workspace arrangement.
- Add a layout-version migration for the new grouping and default front tab.
- Detect an untouched legacy default layout using the full saved layout state, including panel positions, grouping, selected tabs, and collapsed states, before adopting new defaults. Do not infer that a workspace is untouched from dock/order fields alone.
- Preserve custom panel locations, floating positions and sizes, widths, selected tabs, collapsed states, and user grouping choices. In particular, a user who changed only the selected tab or collapsed state still has a customized layout.
- Keep a reset-layout command that explicitly restores the new default.
- Add tests for untouched legacy layouts and layouts customized only by tab selection, collapse state, grouping, dimensions, or floating position. Verify migration is idempotent across repeated loads. Audit any earlier migration steps that could overwrite those states before the new migration runs.

### Tests and acceptance checks

- Verify the default panel order and tab order at 1920 x 1080, 1366 x 768, and 1280 x 720 window sizes; repeat at 100%, 150%, and 200% display scaling where supported, recording the resulting logical viewport size. These are review/test targets, not previously measured results.
- Properties and Effects tab headers remain reachable without column scrolling. At smaller effective viewports, the compact library control and all panel overflow actions remain accessible.
- Layers remains usable and does not consume the entire inspector column.
- Library/Assets/Cast remain accessible in the outer column.
- Every existing panel can be opened from the Window menu.
- Narrow-column tests verify no important row is hidden under a scrollbar.
- Saved custom workspaces survive migration unchanged except for required missing-panel additions.
- Reset Layout produces the new compact arrangement.
- UI screenshots are captured before and after using the same document and window dimensions.
- Check keyboard focus order, focus visibility, menu access, selected-tab feedback, and disabled-control tooltips in both normal and compact layouts.

## Implementation order

1. Audit the partial implementation and current diff, record the baseline, then add or update focused tests and fixtures for transform gestures, pose-aware shadows, and workspace migration. Prove each bug regression detects its faulty behavior before treating it as acceptance evidence.
2. Implement the shared selection hit-testing and cursor behavior.
3. Implement the shadow evaluation alignment and run GPU regression tests.
4. Implement the workspace grouping and panel visual cleanup.
5. Run formatting and compilation checks.
6. Run tool, editor interaction, transform-preview, stage geometry, workspace unit, headless renderer, and dock/layout tests. A tool-only test filter is not sufficient for the full interaction path.
7. Run the broader workspace test suite justified by any failures or shared-path changes.
8. Capture before/after UI images and a pose/shadow comparison.
9. Report changed files, test commands, screenshots, and any remaining limitations.

## Planned validation commands

```text
cargo fmt --all -- --check
cargo check --workspace
cargo test -p buzz-app tools::tests:: --lib
cargo test -p buzz-app tools::transform_preview_tests:: --lib
cargo test -p buzz-app editor::tests:: --lib
cargo test -p buzz-app stage::tests:: --lib
cargo test -p buzz-app app::dock_geometry_tests:: --lib
cargo test -p buzz-ui --lib
cargo test -p buzz-export --test headless_filters -- --nocapture
cargo test -p buzz-ui --test dock_columns -- --nocapture
cargo test -p buzz-ui --test real_layout_migration -- --nocapture
cargo test --workspace
```

Confirm current module names before running filtered commands and record the number of tests actually executed; a filter that matches zero tests provides no evidence. Run the broader workspace suite when shared-path changes or unresolved failures justify it. Report existing baseline failures separately, and avoid unrelated formatting rewrites.

### Validation evidence required for completion

- Record each command, its outcome, and the relevant test counts against the final source revision.
- Record failing-before/passing-after evidence for the reported transform and shadow bugs.
- Require at least one actual GPU run for the shadow acceptance tests. A test helper that skips internally when no GPU is available can still produce a nominal successful test result; inspect the logs and report it as skipped, not validated.
- Keep GPU-executed, GPU-skipped, ignored, and failed test results distinct. If GPU validation is unavailable, leave that acceptance item explicitly unverified.
- Include the UI comparison images, animated pose/shadow comparison, tested viewport/scaling settings, and evidence that custom workspaces survive migration.

The final implementation should be made only after explicit authorization, using GPT-5.6 Luna as requested.


## 4. Large Adobe Animate imports and editable round-trip fidelity (2026-09-22)

### Reported problem and required outcome

Large FLA imports leave much of the document blank and omit objects/symbols. Preserve all assets and the exact layer order, folder hierarchy, animated layer parenting, symbol definitions and instances, masking relationships, transforms, registration points, alpha/color transforms, gradients and their matrices, and timeline structure. Include bitmap fills, text, audio/video dependencies, filters, blend modes, and animation in the compatibility inventory. A visually flattened image is not acceptance for editable structural fidelity.

This is planning only. No failing customer FLA has been reproduced in this review; file size as the direct cause remains unproven. Obtain a representative failing file during implementation, record its Animate version/document type, and compare the same document as compressed FLA and unpacked XFL. Preserve the original.

### What the current source establishes

- In `crates/buzz-import-xfl/src/lib.rs`, `DOMBitmapInstance`, text, video, and sound branches report unsupported content; bitmap fills become flat grey. These omissions can directly explain missing artwork and changed appearance.
- Archive loading can skip unreadable library entries with a tracing warning; later unknown-symbol instances are reported unsupported. A missing definition can therefore remove a whole nested character. Audit archive and parser failures together rather than attributing every blank symbol to size.
- Existing importer code already handles symbol pre-registration, gradients, folder/mask links and layer-follow relationships, with regressions for gradient/placement matrix confusion and parent indices. Retain these fixes. Their presence does not prove fidelity for the reported files.
- `crates/buzz-export/src/xfl/mod.rs` already writes FLA, but its `solid` helper returns no paint for gradients and bitmap fills. It reports further losses including tweens, sound, filters and inverse masks. Export fidelity is a separate, confirmed gap.
- `crates/buzz-doc/src/format.rs` defines a native ZIP containing JSON and a BuzzAnimate MIME type. The user's “.buzzfile” requirement refers to this native document (currently `.buzz`). A native save and an Adobe-compatible artifact are distinct format contracts.

### Investigation and implementation sequence

1. Build a source inventory before conversion: all archive entries, scenes, library definitions (including unused assets), instances, layer/frame counts, media references and dependency edges. Reconcile it with parsed, resolved, saved and rendered content. Distinguish intentionally empty frames, hidden/guide layers and off-stage artwork from lost content.
2. Produce persistent, actionable diagnostics with archive entry, symbol, layer, frame, element and reason. Do not let a skipped entry survive only in logs. Detect dangling references, missing media, parse failures and unsupported constructs; preserve source bytes and explicit placeholders where conversion is incomplete. Never label a partial import fully successful.
3. Complete media and library resolution. Register all definitions before resolving instances; preserve shared definitions rather than duplicating them. Test escaped names, Unicode, folders, relative paths, case collisions and duplicates without unsafe normalization. Handle cycles explicitly. Decode embedded bitmap/media variants, preserve dimensions and alpha, implement bitmap fill matrices and text attributes, and bundle dependencies in native saves and Adobe exports.
4. Audit XML state ownership and shape edges: matrices must belong to their actual group, shape, symbol, bitmap or gradient; handle both empty and paired elements. Verify units, winding, fill/stroke indices, holes, clipping and malformed edge diagnostics. Investigate unsupported compression/media formats and parser depth/size limits against the failing fixture.
5. Preserve structure in the scene model and native serialization: separate folders, mask membership and animated layer-follow links; retain nested timelines, symbol types, registration points, instance timing/loop rules, keyframe spans and blank frames. Audit frame-varying parenting, transforms and scene boundaries. Add missing model fields and versioned migrations rather than flattening away semantics.
6. Implement matching renderer and exporter semantics for alpha/color transforms (including offsets), linear/radial gradients, stops, spread/focal settings and matrices, nested masks, blend modes and supported filters. Ensure stage, thumbnails and export evaluate the same content. Validate mask relationships against actual Animate-authored fixtures; existing writer use of masked layer tags alone is not proof of compatibility.
7. Profile large files independently from correctness: archive enumeration, XML parsing, dependency resolution, media decoding, scene merge and rendering. Bound peak memory, avoid repeated parsing/decoding, use background work with progress/cancellation, and make resource-limit failures explicit. Set performance budgets from measured representative files; never meet them by dropping objects.
8. Extend the existing FLA exporter rather than creating a competing writer. Add editable XFL output where appropriate, media packaging, stable ID/name mapping, gradient/image paints, correct masks and parenting, timelines and color transforms. Reconcile the older root PLAN.md statements that FLA write-back is out of scope with the existing writer and this newly requested scope.
9. Preserve unrecognized source metadata/media for lossless pass-through where valid, with dependency tracking and invalidation when edits affect it. Pass-through does not establish correct rendering or editability; report those separately.

### Native file compatibility with Adobe Animate

The required user outcome is that work saved in BuzzAnimate can return to Adobe Animate with the same assets and editable structure. Implement and verify native save/reopen -> Adobe-compatible FLA/XFL export -> actual Animate open/edit/save -> BuzzAnimate reimport. Renaming the native ZIP is not a solution.

Directly opening the native extension in Animate remains a separate requirement to investigate: verify whether an Adobe-side extension/converter can provide that workflow, or whether an explicit compatibility export is required. Do not describe direct native opening as delivered by an export button. Document the tested Animate versions and document types and the exact user workflow. Features without an Adobe equivalent need an explicit supported representation or a declared incompatibility; optional baking must never be represented as exact editable round-trip fidelity.

Full compatibility is the target, not a present guarantee. Keep any unresolved feature on the acceptance list rather than silently narrowing “all assets” to the currently supported subset.

### Acceptance evidence

- Reproduce the reported large-file failure and reduce individual defects to small fixtures while retaining the full-file regression.
- Cover deep/shared symbols, unused library assets, Unicode names, bitmap instances and fills, text, transparent gradients, nested masks, animated layer parenting, nonzero symbol registration, skew/reflection, and multiple scenes/timelines.
- For each valid fixture, reconcile complete asset/reference counts and compare normalized structure, parenting, mask scopes, matrices, alpha, gradient values and timeline spans. Require zero unexplained omissions and zero unresolved references.
- Compare representative frames rendered by BuzzAnimate and Adobe, including mask edges, nested animation and transparent overlaps. Record image-difference tolerances for rasterization; image similarity alone cannot prove structure.
- Test both directions, including BuzzAnimate-authored content and native save/reopen between import and export. Open/edit/save the exported artifact in actual Adobe Animate. Self-round-trip tests alone can hide matching reader/writer bugs.
- Record source hashes, application versions, duration, peak memory and results for representative large documents. Mark Adobe validation unavailable if the licensed application or fixture is unavailable.
- Add importer, document serialization, exporter and app integration regressions. Include `cargo test -p buzz-import-xfl`, `cargo test -p buzz-doc`, and `cargo test -p buzz-export` alongside focused app round-trip and renderer checks; verify target names before execution.

## 5. Scattered rotation handles and drifting transformation point (2026-09-22)

### Evidence and likely causes

The user reports that points around an object scatter during rotation and that the pivot changes position relative to the object. Initially treat “points” as transform handles; also reproduce in Subselection to distinguish displaced path anchors from a gizmo-only defect.

- Confirmed geometry mismatch: `editor.rs::selection_bounds_drawn` reduces projected object quads to axis-aligned bounds. `stage.rs::draw_selection` outlines object quads but places handles on another screen-aligned rectangle. As rotation changes the rectangle's extents, its handles move independently of the object's corners. `app.rs` and `tools.rs` classify zones from rectangular bounds as well.
- Confirmed distinct pivot paths: `Editor::pivot` uses `scene.pivot_of` for one object; a multi-selection without a stored pivot uses the changing selection-bounds center. Recomputed bounds centers can shift for asymmetric multi-selections. Verify coordinate conversions through inherited layer transforms, nested symbol editing, camera/depth projection and animation before assigning the single-object drift to a specific line.
- The tool already stores drag-start bounds, pivot and applied transform in `Transforming`. Audit all Selection/Free Transform paths and preview/commit callers before changing it. Double application, stale parent inverses or fallback-pivot recomputation are hypotheses, not established root causes.

### Fix plan

1. Capture a failing rotation using an asymmetric shape, a nested symbol and a multi-selection. Log local/edit/world/screen corners and pivot at pointer-down, during preview, after commit and on the next drag. Check whether actual path vertices change shape or only the overlay moves.
2. Introduce shared oriented gizmo geometry: local reference bounds, transformed quad, projected handles/edge midpoints, rotation zones and pivot. Use it for painting, hover and pointer-down classification. Keep conservative axis-aligned bounds only for broad-phase hit tests.
3. Define a stable selection frame for multiple objects and freeze it for the gesture. Retain a single object's orientation through commit and reselection; do not rebuild its gizmo from a fresh screen-space bounding box.
4. Store a single object's pivot in its defined local space and map it through exactly the same evaluated transform chain as its artwork. Keep symbol registration separate from the user transformation point. Define multi-selection pivot lifetime explicitly and invalidate it on actual selection/context changes, not every paint.
5. Resolve and freeze the effective rotation center and drag-start transforms once. In a consistent edit/world space apply `D = T(p) * R(angle) * T(-p)` to the baseline; convert each object's result through its own parent's inverse. Use either baseline evaluation or validated incremental deltas consistently, never both. Preserve existing parenting/tween semantics.
6. For rotation about its own pivot, the pivot's world position and local attachment must remain invariant. When a group rotates about another center, individual pivots travel with their objects while the group center stays fixed. Temporary alternate-corner rotation must not silently overwrite the saved pivot.
7. Derive preview, committed artwork, handles and pivot from the same evaluated snapshot. Support undo/redo, cancel, subsequent rotation and native save/reopen. Avoid lossy transform decomposition for skew/nonuniform scale/reflection; handle singular transforms explicitly.
8. If Subselection anchors actually scatter, fix their local-to-edit mapping and transform application as a separate geometry defect; correcting the bounding box does not close that case.

### Regression and acceptance matrix

- Test 0, 45, 90, 180 and 360 degrees, repeated turns, clockwise/counterclockwise motion and angle wraparound.
- Cover centered/off-center/external pivots; asymmetric art; symbols, nested groups and multiple objects with different parents; skew, nonuniform scale, negative scale and near-singular transforms.
- Cover camera pan/zoom/rotation/tilt, layer depth, nested edit-in-place and display scaling.
- Assert each handle follows its corresponding projected reference corner/edge and that drawing and hit-testing agree within a documented screen-pixel tolerance.
- Assert own-pivot rotation fixes the pivot in world space and preserves its local coordinates; whole-turn geometry returns within numerical tolerance without accumulating drift.
- Verify pointer-down -> many updates -> preview -> pointer-up -> next gesture, cancel, undo/redo, and save/reopen. One completed gesture remains one undo operation.
- Include path-anchor checks and before/after recordings of the reported case. Add failing-before/passing-after tool, editor and stage integration regressions; tool-only tests are insufficient.

## Revised execution priority for this expanded plan

This sequence supersedes the earlier implementation-order list where it conflicts. Preserve the existing shadow/UI requirements and their acceptance checks.

1. Audit the current partial changes; establish failing keyboard/focus, import and rotation fixtures plus the existing shadow/UI baselines. Fix keyboard routing and standard commands from section 6 first so subsequent editing and validation are usable.
2. Fix rotation geometry and pivot consistency together with the shared transform interaction work.
3. Complete import diagnostics, missing assets and structural/rendering fidelity, then native persistence and Adobe export fidelity.
4. Validate the real large-file and Adobe round trips, and resolve remaining compatibility gaps explicitly.
5. Complete the already planned pose-aware shadows and right-side workspace work.
6. Run the applicable checks and capture all acceptance evidence against the final revision. Report incomplete or unavailable validation honestly.

Implementation still requires the user's explicit authorization and the previously specified GPT-5.6 Luna implementation model. This update authorizes no source-code changes.


## 6. Standard keyboard commands, clipboard and Tab reliability (2026-09-22)

### Reported problem and source findings

Ctrl+C, Ctrl+V, Tab and other usual commands do not work reliably. Restore predictable keyboard behavior across the stage, timeline, library, inspectors, text fields and dialogs, including after switching focus or opening another document.

- Confirmed: `crates/buzz-ui/src/command.rs` defines Cut/Copy/Paste bindings. `crates/buzz-app/src/editor.rs` implements an internal scene clipboard and contains command-level tests. These do not prove physical keystrokes reach the commands.
- Confirmed: `app.rs::keyboard_commands` returns early whenever `ctx.memory(|m| m.focused().is_some())`. This treats any focused widget as a text editor and can suppress editor shortcuts after normal UI navigation. Reproduce which widgets retain focus in the reported workflow.
- Repository search found no explicit `Event::Copy`, `Event::Cut`, `Event::Paste` or `Key::Tab` handlers in the application crates. Check the pinned egui/winit adapter's actual input events: native clipboard shortcuts may arrive as semantic clipboard events rather than ordinary key combinations. This is a likely routing gap, not yet a reproduced backend diagnosis. Tab may already be consumed by egui's built-in focus navigation; absence of an app handler alone is not a defect.
- The artwork clipboard is application state. Audit its relationship with the OS clipboard and external text/image paste; do not assume that an internal copy makes content available to other applications.

### Implementation approach

1. Reproduce keyboard failures before changing bindings. Compare menu invocation with physical keys and capture event type, modifiers, focused widget, active editor context, command eligibility and consumption order. Do not log clipboard contents or typed text. Inspect the current native window -> egui -> widget -> app dispatch path and persisted shortcut overrides.
2. Replace the blanket focus gate with explicit command ownership: modal dialogs and shortcut capture first; active text/numeric/code editing next; the active stage/timeline/library context next; eligible application-wide commands last. Respect widget-consumed input and route each event once. Clicking the stage must establish stage ownership; closing a dialog or switching documents must restore meaningful focus without stale capture.
3. Handle native Copy/Cut/Paste events alongside ordinary/custom shortcut bindings where the pinned backend requires it. Deduplicate semantic and key events originating from the same gesture. Text widgets retain normal text copy/cut/paste; artwork and timeline commands operate only in their own context. Preserve existing explicit frame-copy bindings and document the default Ctrl+C/Ctrl+V behavior when frames rather than artwork are selected.
4. Use the same command dispatcher and enabled-state rules for menus, shortcuts and the command palette. Keep saved keymap overrides authoritative, detect conflicting bindings, provide reset-to-default behavior, and show effective shortcuts in menus. Audit Ctrl/Command, Shift, Alt/AltGr, keyboard layouts, pressed/released events, OS repeat and window focus loss. Commands such as paste must not execute twice from one press.
5. Preserve the internal structured artwork clipboard, including dependent symbols, media, transforms, masks and alpha across documents. Define OS clipboard formats and precedence explicitly: native structured data where supported, text for text editing, and supported images for asset paste. Unsupported/external clipboard data must not silently paste stale internal artwork. Report unavailable/empty/unsupported clipboard conditions without losing the current selection. Cut must delete only after successfully creating its intended clipboard payload; each paste/cut is one undoable edit.
6. Make Tab and Shift+Tab reliable focus traversal for controls, with visible focus and a predictable order. Define separate behavior for multiline/code editors that intentionally capture Tab, with an accessible way to leave them. For any desired stage-specific Tab command (such as panel visibility), inspect existing product intent and document the chosen scope before binding it; the user's report does not specify that behavior. Avoid globally intercepting Tab or breaking dialog/inspector navigation. Enter activates or commits the focused control; Escape cancels the active transient interaction before affecting the document.
7. Audit the full standard command set: Ctrl+X/C/V/A, Ctrl+Z, Ctrl+Shift+Z/Ctrl+Y, Ctrl+S/Shift+S, Ctrl+O/N, Delete/Backspace, arrows/Shift+arrows, Enter, Escape, Tab/Shift+Tab and existing playback/tool shortcuts. Apply each only where meaningful; typing, IME composition, numeric editing and rename operations must not trigger artwork deletion, selection changes or tool switching.
8. Integrate focus restoration and traversal with the planned right-side panel redesign, floating docks and compact layouts. Keyboard operation must remain usable after panels collapse, move or close and after an OS file dialog returns.

### Required regression and acceptance evidence

- Establish failing-before/passing-after app event-loop tests, not just direct calls to `Editor::execute` or assertions that a binding exists. Include semantic clipboard events and key events according to the actual backend, and a duplicate-delivery case.
- Copy/cut/paste selected artwork on the stage; verify contents, dependencies, exact operation count and undo/redo. Repeat across documents and after inspector clicks, layer renaming, library interaction, popup dismissal, file dialogs and window deactivate/reactivate.
- Verify text and numeric fields retain their own selection, clipboard and undo behavior without changing artwork. Test focused non-text controls separately; they must not disable all unrelated shortcuts.
- Verify timeline frame clipboard and library behavior in their documented contexts; test empty selection, locked layers, empty/unsupported clipboard and clipboard access failure.
- Test external text/image paste and native artwork clipboard precedence; verify an OS clipboard change cannot unexpectedly replay stale copied artwork.
- Tab/Shift+Tab reach visible controls in order, show focus, skip hidden/disabled items, and work in modal and compact layouts. Validate intentional code-editor Tab capture and exit. Test no accidental document edits from traversal.
- Test default and customized keymaps, conflicts/reset, modifier variants and key repeat. Check shortcut labels match actual bindings and ensure disabled commands explain their unavailable state where appropriate.
- Perform a physical-key smoke test on Windows in addition to synthetic app tests, including native clipboard integration. Record the tested build, keyboard layout and focus sequence; report unavailable native validation as unverified.
- Run focused app keyboard/focus integration tests and relevant UI command/workspace and editor clipboard tests, then formatting/compilation checks. Confirm filters execute nonzero tests and broaden testing only for shared-path changes or unresolved failures.

### Priority and completion

Treat keyboard routing and standard clipboard commands as the first functional repair after baseline capture in the revised execution order. Retain all existing import/export, transform, shadow and workspace requirements. This remains a plan-only update; implementation follows the existing authorization and model requirements. Completion requires working end-to-end gestures, not merely the presence of menu commands or shortcut declarations.


## 7. Existing JSFL commands: compatibility and script workflow (2026-09-22)

### Current workflow and inspected evidence

BuzzAnimate runs JavaScript in QuickJS through `crates/buzz-script`. The Actions panel accepts source, runs it through Run/Ctrl+Enter, shows output, and saves named commands for reuse. The JavaScript prelude exposes a partial Adobe-style `fl.getDocumentDOM()` API backed by Rust host operations. A script edits a working scene copy; successful application is intended to be one undo operation. Existing execution limits and Stop support must remain intact.

Found command collections at:

- `C:/Users/singh/AppData/Local/Adobe/Animate 2023/en_US/Configuration/Commands`
- `C:/Users/singh/AppData/Local/Adobe/Animate 2024/en_US/Configuration/Commands`

The repository itself contained no JSFL files in the initial search. Treat the installed command collections as candidate user scripts, preserve originals, and let the user choose the authoritative collection. The present default configuration search prefers `%APPDATA%/BuzzAnimate/Configuration`, then Animate versions under `%LOCALAPPDATA%/Adobe` with an `en_US` path. An empty preferred directory can hide the intended shelf; make the selected root visible and configurable and support other locales.

Static inspection of the 2024 commands establishes real compatibility gaps; these scripts were read, not executed during planning:

| Script or operation | Required behavior / current gap |
| --- | --- |
| `eyeLeft.jsfl` and related eye/limb commands | Shared `commonVariables.jsfl`, timeline tween creation and live element property edits such as `el.x`/`el.y`; current selection entries are only `{index}` placeholders |
| `claasicTweenOnAllLayers.jsfl` (existing filename) | Reads layer frames, attempts tween properties and menu fallbacks; catches errors, so a successful return can still mean no animation was created |
| `extendFrames.jsfl` | Reads `layer.frames`, selects layers/frame ranges, inserts/removes frames, prompts for duration; those full semantics are missing |
| `addKeyframeOnAllLayersCurrentFrame.jsfl` | Iterates layers and extends timelines before adding keys; validate selection, frame indices and layer targeting together |
| `removeAllLayerParenting.jsfl` / `restoreLayerParentingFromFile.jsfl` | Need parenting read/write plus explicit file operations such as `FLfile.write`/`exists` |
| Symbol pose/rotation commands | Need real symbol instance properties, nested timeline context and transforms that agree with the pivot fixes |

Confirmed limitations in `prelude.js` / `host.rs`: no `Timeline.createMotionTween`, no exposed layer `frames` collection or `currentLayer` property, and only a count argument in `insertFrames`; its Rust host changes the target layer at the current frame. The Buzz-specific `doc.setTween` is not a drop-in replacement for Adobe scripts. `fl.runScript` already loads shared scripts within the configured root. `prompt` currently returns its default and `confirm` returns true; logging these calls does not reproduce interactive script behavior.

### Compatibility implementation

1. Inventory the chosen shelf recursively, including shared helpers and file/media dependencies. Record filenames/hashes, required APIs, document prerequisites, named layers/symbols and expected results. Use JavaScript parsing for the inventory where feasible; text search alone cannot distinguish comments, aliases, dynamically constructed calls or unreachable code.
2. Build an Adobe compatibility matrix with signature, default arguments, return values, error behavior, index/range conventions and side effects for every API used by the shelf. Reference [Adobe's JSFL reference index](https://github.com/AdobeDocs/developers-animatesdk-docs/blob/master/manifest-JSFL-reference.json) and actual behavior in the user's Animate version. Do not infer semantics from method names or speculative fallback properties in a user script.
3. Implement live Document/Timeline/Layer/Frame/Element/Symbol proxies backed by stable scene IDs. Include `currentLayer`, selected layers/frames, frame spans and element collections, writable selection and instance geometry/matrices, symbol playback/first-frame properties, library items and edit-in-place context as required by the inventory. Read-after-write must work inside one script; unsupported property assignments must not silently become detached JavaScript fields that leave the document unchanged.
4. Complete frame insertion/removal/keyframe operations with the full Adobe signatures and exact range/selection semantics. Map classic, motion-object and shape tween APIs and easing independently; do not alias every tween operation to one generic tween kind. Verify locked/folder/mask layers, blank frames, parented layers and nested symbol timelines.
5. Implement translation, rotation, scale, skew, transform points and direct element setters through the same scene transform services used by manual editing. Preserve parent spaces and registration points and reuse section 5 regressions. Include name-based layer/instance targeting used by the character scripts.
6. Complete `fl.runScript` URI resolution, global helper scope and function arguments, plus the `FLfile` and library/media operations actually required. Keep configurable script/resource roots, report missing dependencies with filename/line, and preserve URI/Unicode paths. Support menu-command aliases only when backed by a real command with verified semantics. Keep Buzz-specific extensions visibly separate from Adobe compatibility APIs.
7. Replace automatic prompt/confirm answers in interactive runs with real suspend/resume dialogs that do not block the editor. Headless tests and AI-invoked runs must supply explicit input values or pause for input; never silently accept a default that changes a document's duration. Do not rerun already executed operations just to obtain a prompt response.
8. Add a JSFL shelf UI: choose/import a Commands folder, browse commands, inspect prerequisites/compatibility, run, stop, view errors and bind shortcuts. Preserve source and relative helper folders; loading a shelf must not execute it. Keep existing Actions source editing and saved commands. Add file-open support rather than requiring users to paste every script manually.
9. Keep scene mutations transactional and undoable, with defined handling of selection/playhead/context changes. Errors or cancellation must discard the staged document edit. File writes/export jobs have different rollback semantics: stage writes where feasible and report completed external effects honestly. Maintain time/memory limits, configurable for measured legitimate large jobs, without disabling Stop.
10. Share underlying scene operations between UI and JSFL while preserving synchronous read-after-write inside a script transaction. Do not force every script call through a deferred UI queue; the current scripting module explicitly explains why that breaks scripts.

### Validation and acceptance

The existing ignored `jsfl_shelf_compatibility_report` test is a diagnostic starting point only: it can return successfully when no shelf exists, counts scripts that return without errors, and uses a generic fixture. It cannot certify useful execution or output parity.

- Extend it to report discovered/executed/skipped/failed counts, missing APIs and no-op outcomes separately. Missing fixtures or a zero-script run must not count as compatibility success.
- During implementation, select the root via `BUZZ_JSFL_DIR` and run `cargo test -p buzz-script --lib jsfl_shelf -- --ignored --nocapture`, then `cargo test -p buzz-script`; confirm the current target names. Do not run arbitrary shelf commands against production documents for validation.
- Build representative copies of the user's character documents with the expected names, selected symbols, frames and parenting. Run unchanged scripts against matched starting states in Animate and BuzzAnimate with identical prompt responses.
- Assert actual results: inserted frame counts and spans, tween type/easing and intermediate poses, moved/rotated object transforms, symbol first-frame changes, preserved masks/parenting, helper settings and native save/reopen. Compare outputs after Adobe export too.
- Exercise caught-error/no-op scripts, repeated invocation, absent selection, missing layers/assets, nested includes, prompt cancel, exceptions, timeouts and Stop. Verify one undo restores the document and redo reproduces the successful result.
- Deliver a per-script compatibility report. Every script in the chosen shelf must either pass its intended-action checks unchanged or remain explicitly blocked with an identified fix. If a script also fails in Animate, document that separately; do not modify originals to conceal a host compatibility gap.

## 8. Local AI model controlling the application's actions (2026-09-22)

### Target and model selection

Add an optional local assistant that can inspect the document, execute the user's tested JSFL commands and perform all user-facing application actions through a documented automation interface. Keep the app fully usable without a model. This is a planned integration, not an existing feature verified in this review.

Preferred candidates are the user's Qwen3.8-27B at Q4 and Gemma 4 12B, plus any compatible model available through the user's local runner. The Qwen publisher lists Qwen3.8-27B; Google documents Gemma 4 12B. These establish the model families, not the availability or correctness of a particular installed quantization/runtime combination. Sources: [Qwen official repository](https://github.com/QwenLM/Qwen3.8/blob/main/README.md), [Google's Gemma 4 12B announcement](https://blog.google/innovation-and-ai/technology/developers-tools/introducing-gemma-4-12b/).

Do not hard-code an assumed Q4 tag or silently substitute another model. Discover the runner's actual installed IDs, allow a user-entered ID, and record artifact/quantization, backend version, context window and tool/vision capability. Inspect available RAM/VRAM and benchmark with BuzzAnimate rendering concurrently. Weight size alone is not total memory use; account for context/KV cache, runtime buffers and GPU headroom. Choose context/offload settings from measurements. No model download, installation or benchmark occurred in this planning update.

### Architecture and implementation

1. Add a provider interface for connection health, model listing, streamed generation, structured tool calls, cancellation and errors. Implement a local Ollama adapter first; add local LM Studio/llama.cpp server adapters after confirming their actual API contracts. Expose endpoint/model/context settings and a connection test. Prefer loopback; do not silently send documents to a cloud fallback.
2. Implement an explicit agent loop: send relevant state and available tool schemas, receive proposed calls, validate and execute them, return typed results, then allow the model to continue until completion or its step/time budget. Ollama documents this client-executed tool-call pattern: [Ollama tool calling](https://docs.ollama.com/capabilities/tool-calling). Model-generated text alone must never be treated as a successful app action.
3. Build a versioned action registry shared with UI commands and scripting services. Each action needs a stable ID, argument/result schema, preconditions, supported context, undo behavior and job status where asynchronous. Enumerate both `Command` variants and mutations currently reachable only through panel controls/direct editor methods; a command-enum wrapper alone will not expose all actions.
4. Track complete coverage across documents/save/open/import/export; layers/folders/parenting/masks; selection and geometry; symbols/library/media; frames/tweens/playback; text/styles/gradients/filters; rigging/camera/lights/audio; panels/workspaces/preferences; and saved JSFL commands. Expose typed operations for each. Maintain a machine-checkable action-to-UI/service map; leave any uncovered operation explicitly incomplete rather than promising universal control.
5. Provide read tools for document metadata, selection, stable object/layer IDs, timeline spans, transforms, library dependencies and enabled actions; add filtered/paginated queries for large documents. Offer a rendered preview for models that support images. Models without vision can use structured state and deterministic verification. Keep coordinate spaces and frame units explicit in every schema.
6. Let the AI invoke registered JSFL commands by ID with explicit parameters, input responses and required context. Use the same compatibility report and transactional runtime from section 7. Generated JSFL, if enabled, runs through that runtime with limits and diagnostics, never through an unrestricted shell. Distinguish script prompt input from permission to perform external actions.
7. Validate IDs, types, finite coordinates, frame ranges and command preconditions before applying calls. Bind requests to the document ID and revision; reject/replan stale mutations if the user edits during inference. Use request IDs to prevent duplicate mutations after retries or reconnects. Do not rely on the model to enforce correctness.
8. Offer concise chat, progress, affected-object highlighting, Stop and Undo. Support preview/dry-run and direct execution of authorized reversible edits without repeated confirmation. Group coherent multi-step edits into a defined undo transaction or checkpoints. For overwriting files or other irreversible external effects, follow the app's explicit user authorization and normal save/export rules.
9. Run inference and long jobs off the UI thread; preserve rendering responsiveness and cancellation. Return real job handles/status for imports/exports rather than announcing completion when a job was merely queued. On disconnect, malformed calls, out-of-memory or exceeded budgets, stop cleanly and state which steps committed.
10. Discover and test capabilities per model/backend. If native tool calling is unavailable, optionally accept strictly validated structured action JSON with bounded repair attempts. Reject malformed/unknown calls; never guess actions from free-form prose. Treat document text and imported content as data, not authority to change the user's task.
11. Log tool requests, results and document revisions for debugging, with controllable retention; avoid storing entire documents or unrelated clipboard contents. Keep endpoint credentials out of project files. The default local workflow must remain usable offline after the chosen model is installed.

### Acceptance tests and rollout

- Start with deterministic provider doubles for tool-call routing, schema rejection, stale revisions, duplicate responses, cancellation, rollback/checkpoints, streaming and asynchronous job completion; then test the actual installed models.
- Benchmark each candidate on the same tasks: add 24 frames to selected layers; move a named symbol 50 stage units; create and verify a tween; run an eye/hand command with shared settings; preserve parenting/masks; save/reopen; and export to Animate. Include ambiguity requiring clarification and missing-target errors.
- Assert document postconditions and intermediate animation/render results, not the model's explanation. Record correct-action rate, tool-schema failures, retries, time to first action, total latency, peak RAM/VRAM and UI responsiveness with model artifact/backend/context identified.
- Verify Stop interrupts inference and prevents new actions, Undo restores completed edits, native edits remain responsive, and a disconnected/absent model does not affect ordinary app use.
- Audit every action-registry coverage row before claiming all-app control. End-to-end tests must include actions outside scripting's current scene-only surface, including dialogs/settings and asynchronous import/export.
- Ship in stages: read-only inspection and state queries; reversible edit tools plus tested saved JSFL commands; complete action coverage and asynchronous jobs; then measured model tuning. Keep unsupported actions/models visible.

## Execution-order additions for scripting and local AI

Retain section 6's keyboard-first priority and all previous acceptance requirements. After the baseline audit, begin the JSFL shelf inventory alongside the existing fidelity/transform investigation. Complete JSFL semantics against the repaired scene/timeline/transform services and prove the user's commands before exposing them to AI. The local assistant follows the typed action registry and validated JSFL foundation; it must not conceal unresolved editor/import/script defects by generating workarounds. Preserve the earlier shadow/UI work in the schedule. This remains planning only under the existing implementation authorization/model requirements.


## 9. Mandatory complete local-AI access to every app function and UI control (2026-09-22)

### User requirement and scope

The local AI must be able to discover, inspect, use and change every user-accessible function of BuzzAnimate, including every small button and icon. This explicitly expands section 8 beyond major editing actions and existing command/script APIs. No control may be excluded merely because it is cosmetic, rarely used, contextual, nested in a menu, or implemented directly in a panel.

Coverage includes all toolbar and tool-option icons; menu/submenu/context-menu entries; panel tabs and headers; collapse/expand, dock, float and resize controls; fields, sliders, checkboxes, toggles, dropdowns, color/gradient pickers and swatches; timeline cells, layer controls and frame operations; library/asset items; canvas selection, handles, pivots and drag gestures; scrolling/zoom/navigation; modal dialogs and file pickers; settings, shortcuts, workspace management, import/export options, script commands, help/about controls and any future feature. Read-only information must be inspectable; every change a user can legitimately make must have an equivalent model-accessible operation.

### Implementation requirements

1. Create a complete UI/function inventory by auditing every panel, widget, menu, command, canvas interaction and dialog. Assign stable semantic IDs to controls and actions, including repeated controls bound to stable object/layer/item IDs. Record their location, purpose, value, allowed operations, ranges/options, visibility, enabled state, prerequisites and automation entry point.
2. Expose a live semantic UI tree plus the full capability catalog. The model must be able to enumerate and search controls, read values, discover disabled-state reasons and learn how to reveal hidden controls. Include contextual menus, collapsed panels, virtualized/off-screen lists and modal state without requiring every item to be drawn at once. Use pagination/filtering for large collections.
3. Provide typed equivalents of activate, set value, choose option, focus, open/close, reveal, scroll, drag, resize, dock/undock and canvas manipulation. These operations must call the same validated services and state transitions as the actual UI. Connect direct widget callbacks that currently bypass the command registry. Icon-only controls require meaningful labels and stable IDs; pixel coordinates or guessed icon appearance are insufficient as the sole automation interface.
4. Preserve normal preconditions. A model can unlock a layer, select a target, open a panel or enter an edit context through the same supported operations as a user; it cannot bypass document invariants simply because a control is disabled. Return actionable explanations for unavailable operations. Read-only fields remain read-only.
5. Support full dialog workflows, including selecting file paths, export presets, save options and responding to script prompts. Native OS dialogs need a tested adapter or an equivalent typed application operation with identical validation and results; inability to inspect a native dialog is not grounds to omit its functionality. Keep existing user authorization for irreversible/external effects; full capability coverage does not imply permission to perform unrelated actions.
6. Let any configured model discover the same complete interface. Model size, quantization and vision support must not determine which app actions are exposed. Retrieve relevant capability subsets per request to fit small context windows while keeping the entire catalog searchable. Distinguish complete API exposure from each model's measured ability to use it correctly.
7. Record each operation's actual result, new value, affected IDs and updated UI/document revision. Verify state changes rather than assuming that issuing an activation succeeded. Handle transient/recreated widgets through semantic identity and reject stale targets. Keep Stop, progress, undo/checkpoints and responsive manual interaction throughout multi-step UI workflows.
8. Require all new user-facing controls and features to register their semantics and automation operations as part of implementation. Add build/test checks for unregistered interactive controls and action/catalog mismatches, supplemented by manual audits for custom-painted widgets and gestures. This requirement must remain enforced after the initial integration ships.

### Completion gate: no omitted controls

Maintain a coverage table with one row per control/action family and its repeated/contextual variants: stable ID, UI location, read/query operation, activation or mutation operation, prerequisites, automated test and result. Include non-document UI state such as panel widths, selected tabs, collapsed sections and preferences.

- Every interactive control must have a tested automation path; every displayed value needed to operate the app must be queryable. Mark truly informational/noninteractive elements explicitly with a reason rather than silently dropping them.
- Test actual small icons, overflow menus, context menus, nested dialogs, collapsed/floating panels, narrow layouts, off-screen items, custom canvas handles and controls revealed only for particular selections or document types.
- Compare manual and automated execution from identical initial state for both document changes and UI/settings changes. Validate end-to-end workflows spanning multiple controls, not just direct backend calls.
- Run deterministic coverage tests independently of model quality, then real-model tasks that demonstrate discovery, navigation, inspection, activation and value changes. Report interface coverage and model task success separately.
- Any missing control, unimplemented operation or untested native dialog remains an open completion blocker for the claim that AI can control every function. Do not redefine completion as access to only common actions or existing commands.

This is a mandatory extension of section 8 and its acceptance criteria, not an optional later enhancement. Preserve all earlier work and the plan-only implementation status.


## 10. Detailed local-AI implementation blueprint (2026-09-22)

All names and contracts below are proposed implementation designs, not existing features. This section makes sections 8 and 9 executable as engineering work while retaining their complete-functionality requirement. Implement under the plan's existing authorization and model conditions.

### 10.1 Architecture and ownership

```text
Assistant panel / user request
             |
             v
Agent session coordinator <----> Local model provider worker
             |
     validated action requests
             v
Main-thread automation bridge <----> live UI/control catalog
             |
             +--> App dispatch / Editor services --> document transactions
             +--> workspace/UI state services
             +--> JSFL runtime --> staged script result
             +--> TaskRegistry --> import/export/render jobs
             |
       typed results + revisions + optional preview
             |
             +--------------------> agent's next step
```

The model proposes operations; Rust owns validation, execution, document mutation and success reporting. The inference worker receives immutable snapshots and serializable tool results. It never receives mutable `App`, `Editor`, egui context or renderer references. All live-state changes pass through the main-thread bridge or an existing worker's normal result-application path.

Build on existing seams: `app.rs::dispatch`, `editor.rs::run`, `buzz-ui::Command`, `buzz-ui::Workspace`, `buzz-app::TaskRegistry` and `buzz-script`'s staged scene execution. Audit existing semantics before refactoring. Preserve synchronous read-after-write inside a JSFL transaction and the pinned egui/wgpu versions.

### 10.2 Proposed code layout

| Location | Responsibility |
| --- | --- |
| `crates/buzz-automation/src/schema.rs` | Versioned action/control IDs, typed requests/results, errors, revision tokens and JSON schemas |
| `crates/buzz-automation/src/catalog.rs` | Static capability descriptors, categories, discovery/search and coverage manifests; no editor ownership |
| `crates/buzz-ai/src/provider.rs` | Provider trait, model/capability descriptors, streamed events and cancellation contracts |
| `crates/buzz-ai/src/providers/` | Ollama first; separately tested local LM Studio and llama.cpp adapters |
| `crates/buzz-ai/src/session.rs` | Agent state machine, budgets, conversation/tool history and stop/resume coordination |
| `crates/buzz-ai/src/context.rs` | Context selection, token-budget management and snapshot requests |
| `crates/buzz-ai/src/validation.rs` | Proposed-call decoding and schema validation before main-thread validation |
| `crates/buzz-app/src/automation/` | Register app actions, query state, resolve controls, apply operations, transactions and async job integration |
| `crates/buzz-app/src/ai.rs` | Connect panel events, provider sessions, app wakeups, tasks and result channels |
| `crates/buzz-ui/src/assistant_panel.rs` | Conversation, progress, Stop, change summary, preview and Undo controls |
| `crates/buzz-ui/src/ai_settings.rs` | Local endpoint, installed model picker, connection test and resource settings |
| `crates/buzz-ui/src/automation.rs` | Semantic widget wrappers and per-frame control registration |
| `docs/AI_ACTION_COVERAGE.md` | Generated catalog plus manually audited custom gestures/dialog coverage and test evidence |
| `crates/buzz-app/tests/ai_*.rs` | UI/action parity, transaction, job and full workflow tests |

Keep dependency direction acyclic: `buzz-automation` contains app-independent contracts; `buzz-ai` depends on these contracts but not `buzz-app`; UI emits automation descriptors and user intents without owning model execution. `buzz-app` wires all components. Use existing serde/channel infrastructure where appropriate. Select and pin a maintained HTTP transport after checking current API/MSRV compatibility; isolate inference I/O from the UI thread. Do not pull in an inference engine or model weights merely to connect to an existing local server.

### 10.3 Data contracts and action execution

Define these serializable contracts with explicit schema versions:

- `ActionDescriptor`: stable action ID, description, category, parameter/result schema, required context, state domains it reads/writes, undo policy, asynchronous flag and external-effect classification.
- `ControlDescriptor`: stable control ID, label/role, bound object/item ID, parent container, current value/type/range/options, visibility/enabled/focus state, disabled reason, supported operations, reveal action and UI revision. Screen bounds are optional diagnostics, not identity.
- `ActionRequest`: request ID, agent/session/turn ID, action ID, typed arguments, document ID where relevant, expected state revisions and optional transaction ID. User authorization is attached by the application, never asserted by model-generated arguments.
- `ActionResult`: request ID, status (`completed`, `queued`, `needs_input`, `conflict`, `failed`, `cancelled`), structured value, changed IDs/domains, new revisions, undo/checkpoint reference, optional job ID and typed error.
- `StateSnapshot`: document identity/revision, selection/playhead, relevant layer/object/timeline data, UI/workspace revision and capability version. Track document, UI/settings and task state separately so a harmless UI repaint does not invalidate an unrelated document edit.

Illustrative model-side proposal (exact IDs and schemas to be generated from the registry):

```json
{
  "action_id": "object.translate",
  "arguments": {
    "object_ids": ["object-42"],
    "delta": {"x": 50.0, "y": 0.0},
    "space": "stage",
    "frame": 12
  }
}
```

The host attaches request/document IDs and expected revisions from the snapshot that supplied `object-42`. Validate finite values, existing targets, coordinate/frame units, locks, selection/context dependencies and transaction eligibility immediately before execution. After success return the actual new transform and changed IDs; do not let the model manufacture the result.

Standard errors include unknown action, invalid arguments, missing target, wrong context, disabled control, stale revision, unsupported capability, user input required, cancelled operation and failed job. Return enough structured detail to recover without dumping the entire document.

### 10.4 Complete UI instrumentation

1. Audit every `buzz-ui` panel plus app-owned custom widgets, dialogs, stage tools and context menus. Produce the initial control/action inventory before estimating completion. Include non-command mutations performed directly through panel responses.
2. Introduce reusable semantic wrappers for icon buttons, ordinary buttons, toggles, numeric/text fields, sliders, choices, tabs and menu entries. A wrapper registers the control descriptor and sends both human and AI activation through the same callback/service. Stable IDs must not derive from translated labels, screen coordinates, transient egui IDs or list positions.
3. Example identity: `inspector/object/{object_id}/opacity`. Repeated library/layer controls bind to durable IDs. Separate a control's identity from the current value and screen position. Icon-only controls receive labels such as 'Toggle layer lock'.
4. Maintain two views: a full catalog describing available control families and a live tree describing current state. Hidden/virtualized controls are resolved from their data models; register how to reveal them without rendering all rows. Context menus expose their items once the target/context is known.
5. Support query, activate, set, choose, reveal, focus, scroll, dock, resize and gesture tools. A generic `ui.set` validates against that control's type and allowed operations; it is not arbitrary property reflection. Preserve intermediate events where they matter, such as commit-on-Enter and drag begin/update/end.
6. For custom canvas handles, expose semantic targets and coordinate spaces, with the repaired transform math shared with human gestures. For docking and panel resize use validated layout operations and compare resulting layout with manual interaction.
7. Native file pickers get equivalent typed app operations for paths/options or a tested native adapter. Script dialogs return `needs_input` and resume the same suspended transaction. Opening one modal must prevent incompatible actions until it is answered or closed.
8. Enforce registration through component conventions and tests. Static checks are a backstop, not a proof that every custom-painted icon was found. Run a manual panel/menu/gesture audit and dynamic traversal at representative window sizes to close the remaining gaps.

### 10.5 Provider connections and model configuration

Settings workflow: choose local runner -> configure endpoint -> Test connection -> list available models -> choose exact model ID -> inspect detected capabilities -> run a tiny structured-action probe -> save settings. Provide custom model IDs for runners whose listing is incomplete. Health checks must not download models or start a cloud session.

Persist provider/model settings in app configuration, with migrations and defaults, separately from document content. Keep optional credentials in the appropriate local credential facility. Normalize provider differences into common internal events: text delta, completed tool proposal, usage, end-of-turn, error and cancellation. Never execute incomplete streamed tool arguments.

Use provider-native tool calling when the selected backend/model supports it. Otherwise offer explicitly tested schema-constrained action JSON. Separate reasoning text from executable proposals. Validate installed model templates, quantization and actual tool output rather than assuming capability from a family name. Qwen3.8-27B Q4 and Gemma 4 12B remain preferred candidates; user-selected alternatives use the identical action interface.

Keep inference in an external local runner initially. A dedicated transport worker/runtime handles requests, streaming and cancellation; bounded channels carry events to the application and trigger event-loop wakeups. Add connect, idle-stream and total-turn timeouts and bounded queues. Reuse connections where supported. Stop aborts transport where possible; discard late responses using session generation IDs even if the server cannot cancel its computation immediately.

Record RAM/VRAM, GPU offload, quantization, context size, loaded model size and editor performance in a local diagnostics view. Benchmark while rendering a realistic project; reduce context/offload or choose another available model if needed. Preserve the user's model choice and report limitations rather than silently switching.

### 10.6 Agent loop and context management

Session states: `Idle -> GatheringContext -> Generating -> Validating -> Executing -> Observing -> Generating`, ending in `Completed`, `Failed` or `Cancelled`. `WaitingForInput` and `WaitingForJob` are explicit resumable states. Every transition has a session/request identity and can update the assistant panel.

For each user task:

1. Capture the relevant document/UI revision and user's requested scope. Obtain a compact scene/selection summary and search the complete capability catalog for relevant tools.
2. Send the task, actual available tools, schemas and state to the selected local model. Supply units, preconditions and short API examples; never tell the model that an unsupported action exists.
3. Accumulate the response until each tool call is complete. Decode and validate it, then submit it to the main-thread bridge. Perform independent reads concurrently when safe; serialize mutations and dependent operations.
4. Recheck revisions and preconditions, execute, and return authoritative results. If stale, obtain fresh state and replan. If the task is ambiguous, request only the missing user choice and retain the session.
5. Observe changed values or job results. Feed the outcome back to the model for the next step, including actionable errors. Bound retries and detect repeated equivalent failing calls or no-progress loops.
6. Finish only after verifying the requested postconditions or honestly reporting what remains incomplete. Produce a short user-visible change summary linked to real action results and undo entries.

Expose small discovery tools (`capabilities.search`, `capabilities.describe`, `state.query`, `ui.query`) so all functionality remains reachable without sending thousands of schemas in every prompt. Load detailed action families on demand. Page large layer/library lists and retrieve objects by stable IDs. Summarize older action history but retain unresolved errors, user constraints, document revisions and transaction state. Never truncate tool arguments/results mid-structure. Count or conservatively estimate tokens with reserved space for tool results.

Optional preview images come from the normal renderer at a bounded resolution and retain an explicit mapping to stage coordinates. Vision is an aid; text-only models still access every semantic control and document function.

### 10.7 Transactions, Undo, user edits and long jobs

Do not keep a live mutable document transaction open during an inference request. Stage a bounded, reversible edit batch against a snapshot, validate it, then commit on the main thread if relevant revisions still match. Where an action cannot run on a clone, use explicit checkpoints and serialize its application. Do not wrap existing independently undoable commands in a fake group without refactoring their mutation services.

Coherent batches such as adding keys and setting a tween become one undo operation where supported. Multi-stage tasks show named checkpoints. Manual edits remain allowed between AI batches; stale pending changes must rebase through validated operations or be rejected, never overwrite newer work. UI layout/preferences changes use their own revert snapshots where document Undo does not cover them.

Assign idempotency/request IDs and retain a bounded result ledger. Retrying a timed-out response must first query its status; do not blindly repeat paste, insert-frames or export operations. Stop prevents further queued actions, cancels active jobs where supported and discards uncommitted edits. Already committed checkpoints remain visible with Undo available.

Integrate import/export/render operations with the existing `TaskRegistry`: queue through their real service, return job IDs, stream progress and wait for terminal outcomes. Stage exported files and commit destinations through normal save rules. Document Undo cannot reverse arbitrary external file effects; report them accurately. Inference/session completion must not imply queued jobs succeeded.

### 10.8 JSFL integration and reuse

Register each validated saved JSFL command with its stable command ID, source hash, required document context, optional parameter/input schema and compatibility status. Preserve the original source. An AI `scripts.run` call resolves the registered command, constructs `ScriptContext`, supplies explicit inputs, and runs against the existing staged scene runtime with timeout/memory/Stop controls.

Reuse the dependency resolver for `commonVariables.jsfl` and other includes. Return trace, errors, actual changed state and undo results. Script results require the same revision check as any other asynchronous edit. Generated scripts remain a separate visible operation with source inspection and the same sandbox limits; they do not replace the complete typed control catalog. A user script that requires missing APIs remains blocked until section 7 resolves them.

### 10.9 Assistant user interface

Add a dockable Assistant panel and a compact entry point in the existing panel/menu system. It contains task input, model/connection indicator, streamed concise output, current action, progress, Stop, required-input controls and a result summary. Show detailed tool arguments/results in an expandable activity view, not as mandatory clutter.

Support selecting referenced objects from the result list, previewing a proposed batch, applying authorized edits, Undo of the latest AI checkpoint and retry from fresh context after failure. Keep ordinary reversible work free of repeated approval prompts. Display external effects before execution when normal user authorization requires it. User corrections cancel or revise pending work while preserving completed work and context.

Add settings for endpoint/model, context/output limits, task budgets, local logging and resource behavior. Connection failure is inline and recoverable; it must not block document editing or produce a startup modal every time the app launches without a runner.

### 10.10 Work packages and exit criteria

| Package | Deliverable | Exit evidence |
| --- | --- | --- |
| AI-0: baseline | Full control inventory, action/service map, model setup/hardware discovery and current automation gaps | Reviewed coverage manifest; no inferred claim of full access |
| AI-1: contracts | `buzz-automation` schemas, stable IDs, descriptors, errors and generated catalog | Schema/version tests and deterministic registry validation |
| AI-2: execution bridge | Main-thread routing, state queries, revision checks, reversible batches and result ledger | Manual/tool state parity, duplicate/stale-call and undo tests |
| AI-3: complete controls | Semantic wrappers, all panel/menu/gesture/dialog adapters and live UI tree | Every coverage row has tested read/operation paths; custom UI audit complete |
| AI-4: local transport | Provider abstraction, Ollama integration, model discovery, settings and cancellation | Fake-server tests plus successful local capability probe |
| AI-5: agent loop | Context retrieval, streamed tool handling, bounded execution and observation | Deterministic multi-step tasks, stop/no-progress/error tests |
| AI-6: JSFL/jobs | Validated shelf commands, prompts, import/export/render jobs and result observation | Real fixture outcomes, job completion and rollback/checkpoint evidence |
| AI-7: assistant UX | Panel, settings, progress, previews, input requests and Undo | Keyboard/focus/layout integration tests from sections 3 and 6 |
| AI-8: backend expansion | Tested LM Studio/llama.cpp adapters and selected alternative models | Provider conformance suite and same-task comparisons |
| AI-9: acceptance | Complete control coverage and preferred-model benchmark suite | Published coverage, actual model success/latency/memory results and limitations |

Dependency order: AI-0 -> AI-1 -> AI-2; UI instrumentation AI-3 and transport AI-4 build on these contracts; AI-5 joins them; AI-6 depends on the corrected JSFL/import/export services; AI-7 integrates throughout; AI-8 follows the provider contract; AI-9 closes the work. An early read-only or partial editing milestone is useful progress but does not satisfy the all-functionality requirement. Do not replace the existing keyboard, transform, fidelity and shadow fixes with AI work.

### 10.11 Validation, observability and completion evidence

Add contract and provider tests for malformed/truncated streams, multi-call turns, wrong schemas, unknown IDs, Unicode values, numerical bounds, connection errors, retries and cancellation. Add main-thread integration tests for focus/modal state, contextual targets, hidden controls, manual edits during inference, document switching, transaction rollback and duplicate deliveries. Verify UI/tool parity for every action family and representative repeated-control instances.

Provide deterministic fake-model transcripts that complete real tasks without an installed model, then opt-in real-model tests that report unavailable runners/models as skipped rather than successful. Include actual physical UI runs for native dialogs, docking and canvas gestures. Add full workflows combining frame extension, tween creation, object movement, JSFL eye/hand commands, mask/parent preservation, panel/settings changes and Adobe export.

Planned commands after the new crates/targets exist: `cargo test -p buzz-automation`, `cargo test -p buzz-ai`, targeted `buzz-app` automation/AI integration tests, relevant `buzz-ui` and `buzz-script` tests, `cargo fmt --all -- --check`, and `cargo check --workspace`. Confirm targets and nonzero executed test counts; broaden regression testing for affected shared services.

Deliver schema/catalog version, tested source revision, coverage manifest, test counts, before/after document assertions, runtime/model artifact IDs, measured latency/memory/UI responsiveness and outstanding failures. Keep three independent results visible: completeness of exposed app functionality, correctness of the execution bridge, and each model's task success. Complete access must reach every control; no model is to be described as infallible merely because all controls are exposed.
