# Colour effect on objects like Animate (2026-11-04)

User: "I am not able to set brightness of an object or other effects like alpha as I can do
with a layer — make it work."

Animate: Properties ▸ Color Effect ▸ Style = None / Brightness / Tint / Advanced / Alpha on a
symbol instance (also keyframable and tweened by Classic/Motion tweens; Advanced = per channel
% and offset). BuzzAnimate has layer-level effects; per-object colour effect is missing or
not reachable.

## Status

| Step | What | Who | Status |
|---|---|---|---|
| O1 | Find what exists (object `color_transform`/colour effect fields, the "colour-effect instances" mentioned in BIMARI5_LAG_PLAN, FLA import of `<color>`), then make **Properties ▸ Color Effect** for a selected instance (and group/shape where Animate allows: instances only in Animate — match Animate) with Style None/Brightness/Tint (colour + %)/Advanced (R/G/B/A % and offset)/Alpha, live preview while dragging the slider, keyed per keyframe, interpolated by Classic and Motion tweens, saved/loaded, FLA import/export round trip, lights/caching unaffected (no lag: the renderer path already handles colour-effect instances — check the stamp/cache rules). Multi-selection sets all. Gate: headless tests (set each style, save/load, tween midpoint value, FLA export has `<color>`), a render sheet of one character instance at Brightness −50/+50, Tint red 50 %, Alpha 30 %, Advanced | Sonnet → Opus review | built, awaiting Opus review: UI Advanced + multi-select, FLA export `<color>`, tests (xfl round trip, tween, render sheet scratchpad o1_sheet.png) pass |
| O1 review | Opus 2026-11-04, `scratchpad/o1_sheet.png` looked at (None / Brightness −50 / +50 / Tint red 50 % / Alpha 30 % / Advanced on one instance — all render as expected): **accepted for hand testing.** Most already existed (ColorTransform, tween lerp, save/load, renderer, FLA import); added the Advanced editor, multi-selection, FLA export `<color>`; tests pass (xfl round trip, tween midpoint, GPU pixels). Notes: the chosen style "Advanced" for a transform that equals a named style resets after restart; multi-select shows the first instance's values; not checked in real Animate; no frame-cost measurement (no renderer change) | Opus review | accepted with notes |
