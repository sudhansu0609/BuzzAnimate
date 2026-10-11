# FreeTransform marquee (overlap) + dashed Selection-tool outline vs solid FreeTransform outline

Two refined changes per your answers. (1) Free Transform: a click-drag becomes a marquee that selects every object the rectangle *overlaps* — reuse the existing `PickInRect` → `objects_in` pipeline and relax its test from `contains_rect` to `overlaps`. (2) The two tools read differently through the selection line on the stage: with the Selection tool active the per-object selection outlines are drawn as fine small dashes (manual dash segments, matching the existing hand-drawn-dash pattern in `timeline_panel.rs`), while Free Transform keeps its solid outlines. This replaces the earlier toolbar-button styling step.

## Steps

1. Marquee semantics → overlap: in crates/buzz-app/src/editor.rs `objects_in_frame` (~line 2960) change `in_layer.contains_rect(bounds)` to `in_layer.overlaps(bounds)` (Rect::overlaps already exists — used at editor.rs:1891), and update the `objects_in`/`objects_in_frame` doc comments ('fully inside' → 'overlap'). Note this also changes the Selection tool's marquee identically, which matches the requested behaviour.
   - files: crates/buzz-app/src/editor.rs
   - verify: cargo test -p buzz-app — expect the existing PickInRect tests at editor.rs:10847–10913 to reveal any assertions written for the old 'fully inside' rule; update those assertions to overlap semantics.

2. FreeTransform marquee preview: in crates/buzz-app/src/tools.rs the `marquee()` closure (~line 872) currently forces `Preview::None` for FreeTransform. Remove that special case so a FreeTransform drag previews `Preview::Marquee(Rect::from_points(origin, current))` like Selection does.
   - files: crates/buzz-app/src/tools.rs
   - verify: cargo build -p buzz-app; manual: drag an empty area with Free Transform — rubber band must track the pointer.

3. FreeTransform marquee commit: in crates/buzz-app/src/tools.rs `pointer_up` FreeTransform arm (~line 1162), the early-out `let Some(bounds) = ctx.selection_bounds else { return PickAt{point:end, additive:false} }` fires for drags too. Change to: when `!was_click` return `ToolAction::PickInRect { rect: Rect::from_points(origin, end), additive: mods.shift }`; only return `PickAt` when `was_click`. Before editing, read the unread tail of this match (~line 1229+) — if a non-click `TransformZone::Inside`/catch-all arm already exists and returns `None`, route it to `PickInRect` as well so a drag starting inside an existing selection's box also marquees.
   - files: crates/buzz-app/src/tools.rs
   - verify: cargo test -p buzz-app — `free_transform_can_still_select_another_object` (click path) and `free_transform_leaves_an_existing_selection_alone` must still pass; add a test next to tools.rs:3336 asserting a FreeTransform non-click drag with no selection returns `PickInRect` (and shift ⇒ additive: true).

4. Dashed selection line for the Selection tool: in crates/buzz-app/src/stage.rs `draw_selection` (~line 609), add a small helper that walks a closed screen-space polyline and emits short alternating segments (e.g. ~3px dash / ~3px gap — the manual-dash pattern already used at timeline_panel.rs:2268, since egui `Shape::line` has no dash support). In both the per-object outline (lines 634–637) and the fallback selection-bounds outline (lines 654–657), draw dashed when `editor.tool() == buzz_ui::ToolId::Selection` and keep the existing solid `egui::Shape::line` for Free Transform, Subselection and any other tool.
   - files: crates/buzz-app/src/stage.rs
   - verify: cargo build; manual in the app: select an object with Selection (V) → fine small dashes on its outline; switch to Free Transform (Q) with the same selection → solid outline; back to Selection → dashed again. Confirm handles/pivot chrome are unaffected.

5. Add/adjust tests: (a) tools.rs — FreeTransform drag ⇒ PickInRect, drag+shift ⇒ additive, click ⇒ still PickAt; (b) editor.rs — a marquee overlapping a large object only partially selects it (overlap rule, replacing any fully-inside assertion).
   - files: crates/buzz-app/src/tools.rs, crates/buzz-app/src/editor.rs
   - verify: cargo test -p buzz-app -p buzz-ui

## Risks
- `objects_in_frame` is shared: relaxing to overlap also changes the Selection tool's marquee (intended, but any test asserting 'fully inside' must be updated, not just added to).
- The unread tail of the FreeTransform `pointer_up` match (~tools.rs:1229+) may already handle non-click Inside drags — editing without reading it could double-emit or conflict; step 3 reads it first.
- A degenerate marquee (drag of ~0 px) may overlap nothing, so a tiny accidental drag selects nothing instead of clicking under it — acceptable, but note the click path (PickAt) remains the way to pick by point.
- Dashes are drawn in screen pixels: at very low zoom the 'fine' dashes stay fine on screen (as requested), at high zoom they look proportionally smaller than the solid box — that is the intended look, flagging in case you prefer zoom-relative dashes.

## Open questions
- Confirmed interpretation: the distinction between the two tools is in the *stage* selection line (Selection = fine dashes, Free Transform = solid), and the earlier idea of changing the toolbar buttons' active styling is dropped — correct?
