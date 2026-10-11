# Distinct active state for Selection/FreeTransform + marquee selection with FreeTransform

Two changes: (1) make the Selection and FreeTransform toolbar buttons read more distinctly when active by special-casing them in the shared `tool_button` helper in buzz-ui, and (2) make a click-drag with the FreeTransform tool act as a marquee that selects everything inside the dragged rect, reusing the existing `ToolAction::PickInRect` → `Editor::objects_in` pipeline that Selection already uses. No new data model needed — the editor already implements the marquee pick; only the tool state machine and preview suppress it for FreeTransform today.

## Steps

1. Visual active state: in crates/buzz-ui/src/panels.rs `tool_button` (line ~724), special-case `matches!(tool, Selection | FreeTransform) && active` to use a stronger active style than the generic `Palette::active()` fill — e.g. an accent fill (add one to `Palette` in buzz-ui if none exists that is clearly stronger than `Palette::active()`) plus the already-bright ink, and optionally a 1px stroked border on `response.rect`. Keep hover behaviour unchanged for other tools.
   - files: crates/buzz-ui/src/panels.rs, crates/buzz-ui/src/palette.rs (only if a stronger Palette color must be added)
   - verify: cargo build -p buzz-ui; run the app and click Selection (V) then Free Transform (Q) — both must read as clearly 'on' against the dim inactive buttons. Also confirm no other tool's inactive look changed.

2. FreeTransform marquee preview: in crates/buzz-app/src/tools.rs (~line 872), the `marquee()` closure currently returns `Preview::None` for FreeTransform so a drag shows no rubber band. Remove the FreeTransform special case so FreeTransform drags preview `Preview::Marquee(Rect::from_points(origin, current))` exactly like Selection. Confirm the arm at ~line 1229+ (drag starting on `Inside`/outside with an existing selection) — if it currently falls through to a marquee already, only the no-selection path needs the fix; if it returns `None`, route non-click drags there to `PickInRect` too.
   - files: crates/buzz-app/src/tools.rs
   - verify: cargo test -p buzz-app; manual check: with nothing selected, drag an area with Free Transform — a marquee rubber band must track the pointer.

3. FreeTransform marquee commit: in crates/buzz-app/src/tools.rs `pointer_up` FreeTransform arm (~line 1162), the early-out `let Some(bounds) = ctx.selection_bounds else { return PickAt { point: end, additive: false } }` fires even for drags. Change it to: if `!was_click`, return `ToolAction::PickInRect { rect: Rect::from_points(origin, end), additive: mods.shift }`; only fall back to `PickAt` when `was_click`. The editor side already exists — `ToolAction::PickInRect` in `Editor::apply` (editor.rs ~1954) selects via `objects_in(rect)`, and shift already means additive there.
   - files: crates/buzz-app/src/tools.rs
   - verify: cargo test -p buzz-app (existing FreeTransform tests at tools.rs:3600-3690 and editor.rs:14841-14880 must still pass — in particular `free_transform_can_still_select_another_object` and `free_transform_leaves_an_existing_selection_alone`); add a test next to the Selection marquee test at tools.rs:3336 asserting `pointer_up` of a FreeTransform drag with no selection returns `PickInRect`.

4. Add/extend tests: (a) buzz-app tools.rs — FreeTransform drag with no selection ⇒ `ToolAction::PickInRect`; FreeTransform drag with shift ⇒ additive; click (origin==end) still ⇒ `PickAt`; (b) buzz-ui — if a Palette entry was added, assert it renders (existing `every_tool_has_a_name_and_a_glyph` suite shows the pattern).
   - files: crates/buzz-app/src/tools.rs, crates/buzz-ui/src/tools.rs
   - verify: cargo test -p buzz-app -p buzz-ui

## Risks
- Ambiguity: 'look different when active' could mean the two buttons' *icons* should be more distinguishable from each other (free_transform icon in buzz-ui/src/icons.rs:236) rather than the active-state styling — the plan assumes active-state styling; if icons were meant, step 1 changes to the icons module instead.
- The unexamined tail of the FreeTransform match (tools.rs line ~1229+) may already handle some drag paths; the plan's step 2 verification must read it first so the new marquee doesn't double-emit.
- `objects_in(rect)` containment semantics (fully-contained vs intersecting) haven't been confirmed — if the user wants 'anything touching the area', that helper in editor.rs needs a second predicate.
- Changing the shared `tool_button` risks affecting the two color wells or other panels if the accent color is picked up elsewhere — scope the special case strictly to the two tool ids.

## Open questions
- 'Look different when active' — do you mean the active state should stand out more (brighter fill/border) on the two buttons, or that the Selection and Free Transform icons themselves should be visually distinguishable from each other?
- For the marquee: select objects fully inside the dragged area, or anything it overlaps? And should shift-drag add to the existing selection (the plan assumes yes, matching Selection tool)?
