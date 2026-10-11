# Plan: lag on large rigged, lit scenes (ep1.buzz)

Reported 2026-10-06: "the app is still lagging, it lags so much on large
scenes". The open document was the overnight episode
`B:/youtubeProjects/Originals/series/ep1/buzz/episode/ep1.buzz` (7 scenes,
4–5 image rigs each, `lit: true` from the episode builder, 1525 frames in
Scene 4). Companion: `CHARACTER_TRACE_FIDELITY_PLAN.md` (fewer, cleaner
shapes per rig), `BIMARI5_LAG_PLAN.md` (the unlit import case, whose caches
this scene never reaches).

**Target (unchanged):** every frame ≤ 25–30 ms end to end in the editor,
idle editor at 0 re-encodes, with the same pixels the live draw produces
today. No level of detail, no quality knob.

**User decisions 2026-10-06:** (1) "remove the rim glow" — set-up scenes no
longer put a rim on the key (`buzz-act/src/staging.rs`, `key.rim = 0.0`,
storm included; the Lights panel slider still exists); the open ep1 document
keeps its rim until its Key light's Rim is set to 0 or the episode is
rebuilt. (2) "we can have one light at a time if required" — a scene may be
lit by the key alone if the fill measurably costs frame time
(`FRAME_COST_ONE_LIGHT=1` measures it). **Measured: it does not.** Same
frame, key + sky vs key alone: Flat 38.4 → 37.8 ms, NoModelling 87 → 84 ms,
Full 760–1468 → 682 ms (within the run-to-run swing). The sky fill costs
≤ 1 ms; the time is in the rim, the bands and the shadows, so the scenes
keep both lights and the rim goes instead. **(3) Wrap-up decision (Fable, to
be confirmed by the user): set-up scenes now light with `EdgeMode::Off`** —
the figure bands are rebuilt per pose (boolean offsets of the whole
silhouette), and with brush-outlined rigs that was 8.4 s for the first moving
frame and 37 vs 8 ms for a held one (episode_v2, 2026-10-06 evening). The
Lights panel can switch a document's bands back on. L5 (cheap per-pose bands
from part outlines) is the way to have both.

## Status

| Step | State | Date | Evidence |
|---|---|---|---|
| Diagnosis | done (Fable, measured headlessly + real log) | 2026-10-06 | below |
| A/B: lit vs edges-off vs unlit | **done** — lighting is 99.8 % of the frame | 2026-10-06 | `--layers 0` warm: lit **1468 ms**, edges off **5131 ms** (!), unlit **3.50 ms**. Pipelined `a` 0–120: lit 32 of 121 frames shown, render p50 47 ms, 89 dropped, frame 0 3563 ms; unlit all 121 shown, render p50 6.5 / p95 10.4 ms, encode thread p50 1.4 ms, 0 dropped, 5.2 MB encodings. Detail/edge ladder next (`FRAME_COST_DETAIL`, `FRAME_COST_EDGES`). |
| L0 layer silhouette + rim glow cached; rig silhouette from part outlines | **done as specified, but it does not move the headline** (Sonnet implementer; Opus review pending) | 2026-10-06 | `DrawCache.layer_fx` (`LayerFxMemo`: key = layer id + signature hash of every object (stored `Arc` address, or for a tweened / modifier-posed rig its `pose_fingerprint` + artwork addresses), eval `prepend`, breath, library `content_id`, rim reach/colour, `format!("{:?}", layer.filters)`, and `elapsed`+`stage_frame` only if an animated symbol is inside; value = `Arc<buzz_fx::Painted>` (the silhouette itself is not kept: megabytes); max 2 entries per layer, unused for 2 frames dropped; `Weak`s pin every hashed address). `DrawCache.silhouette` (`SilhouetteMemo`): a rigid part is recorded once per `part.artwork` as leaves `(transform chain, path, stroke width)` and replayed as `world * part.transform * chain * path` in the live walk's own multiplication order (bit-identical); **strokes are outlined after placing, per use** (a stroke width is in document units, so the brief's identity-space outline is not exact); skinned parts, parts with an animated symbol, and any gather under a breath walk live. Kill switch `BUZZ_NO_LAYER_FX_MEMO=1` (OnceLock) and `DrawCache::set_layer_fx_memo(bool)` (hidden). Tests (`buzz-render/tests/rig_lag.rs`): `a_held_rig_layer_gathers_and_builds_its_rim_once`, `a_changed_pose_rebuilds_the_rim_from_the_parts_memo` (0 live shape visits, 0 re-records), `a_tweened_rig_layer_is_keyed_on_its_pose`, `only_an_animated_symbol_moves_the_rim_with_the_playhead`, `the_kill_switch_keeps_nothing`, **`the_memo_does_not_change_a_pixel`** (GPU: sun rim 0.6, `EdgeMode::Figure`, 3 rigid parts x 30 shapes incl. 2 stroked + 1 mirrored, memo cold / hit / re-posed / back vs memo off, 1x and 4x: byte-identical; it fails when the replay skips `wound_forward`, so it can see the mirror). **Measured (ep1 Scene 4 frame 0, `--layers 0`, warm): 1466 ms after vs 760-1468 ms before: unchanged. Pipelined `a` 0-120: 31 of 121 shown, render p50 46.6 / p95 61.6 ms, 90 dropped (before 32 / 47 / - / 89); memo off 30 shown, p50 47.3 / p95 62.3, 91 dropped.** See "What the L0-L2 measurement showed" below. |
| L1 stable posed parts + owner identity | **done** (Sonnet implementer; Opus review pending) | 2026-10-06 | `ArmatureData::posed()` memoised in `PosedMemo` (fingerprint = skeleton root + every bone parent/length/rest/angle/offset + every part artwork address + binding incl. skin weights; memo holds the artwork `Arc`s so an address cannot be reused and `make_mut` must copy); `Clone` = empty memo, `==` always true. Renderer: armature parts get `owner = Some(&part)` **only when the rig itself is a stored object** (tweened / modifier-posed rigs stay owner-less: fresh parts every frame, a cache keyed on them could only defer builds that never hit). Tests: `rig::tests::{posed_twice_gives_pointer_equal_parts, a_changed_pose_or_skeleton_gives_new_parts, a_cloned_armature_does_not_share_the_memo_but_is_equal, the_memoised_pose_equals_a_fresh_one}` (buzz-scene lib rig: 24 passed), `buzz-render/tests/rig_lag.rs::{a_held_lit_rig_builds_its_crescents_once, a_moved_bone_rebuilds_the_crescents, a_rig_with_no_stored_object_is_built_inline_not_deferred}` (new `LightCache::builds()/inline_spent()` `#[doc(hidden)]` counters); `cargo test --release -p buzz-render`: lib 105 passed / 1 ignored, every integration file green. |
| L2 rig figure content hash from the pose, not `elapsed` | **done** (Sonnet implementer; Opus review pending) | 2026-10-06 | `figure_content(object, ctx, &mut AnimatedMemo)`: a rig hashes every bone number (root, parent, length, rest, angle, offset), every part artwork address + transform + binding kind; `content_id` for anything with an instance; `elapsed` and `stage_frame` **only if an animated symbol (length > 1, any depth) is inside** (`AnimatedMemo` in `DrawCache`, per library identity, held like `SymbolTable`). Applies to a bare instance and to groups of instances as well. A member's breath is folded into the figure key (it deforms the artwork with no change of address). Tests `buzz-render/tests/rig_lag.rs::{a_held_rig_of_static_symbols_reuses_its_figure_on_the_next_frame, a_rig_with_an_animated_symbol_rebuilds_its_figure_as_it_plays, a_changed_pose_misses_the_figure_cache}`; `cargo test --release -p buzz-render` all green (lib 105 / 1 ignored, rig_lag 6). |
| L0–L2 Opus review | **passed (code + tests); frame time carried to rim removal, L7, L3** | 2026-10-06 | Re-ran `cargo test --release -p buzz-render --test rig_lag` 12/12 (incl. the GPU pixel-identity test), `-p buzz-scene --lib rig` 24/24, and the buzz-ai suite (38 passed) on the rebuilt workspace. Accepted the implementer's three deviations (owner only for stored rigs; whole-skeleton fingerprint; strokes outlined after placing) — each keeps pixels exact. Its measurement stands: the figure bands fell 1117 → 8 ms warm, but the frame did not move because the rim glow is 13 M lines a pass to *draw* and the ladder runs it twice. **No-rim A/B (my run, `FRAME_COST_NO_RIM=1`, `--layers 0`): 386 ms warm, hide-one deltas ~300 ms each for every layer** — still non-additive, so the ladder still oscillates without the rim; L7 next, then the single-pass cost is measured per rung. Same run, no rim, `BUZZ_NO_LAYER_FX_MEMO=1`: **98.9 ms** — not the memo's doing (with no rim there is no layer fx to memoise); the ladder's pass count differs run to run, so 99 ms ≈ one Full pass without the rim and 386 ≈ four. **Pipelined playback 0–120, no rim: render p50 46.4 ms, 82 of 121 shown, 39 dropped, final buffer scale 1** — the same ~46 ms as with the rim, because in playback the ladder settles at `Flat` (tint only, no bands, no shadows, no rim) and *that* path costs ~40 ms: 530 stamp refusals, every lit shape walked and recoloured live. So after the rim, the lever for playback is **L3** (stamps under uniform light), and for the idle Full frame L3 plus the shadow caster. |
| L7 detail ladder: never step up into a refused level; never end over budget | **done** (Sonnet implementer; Opus review pending) | 2026-10-06 | `DrawCache` (buzz-render `document.rs`): per-frame `measured_lines` / `measured_segments` `[Option; 4]` (indexed by `LightDetail::index`, cleared in `begin()` and on any scale change: growth, `set_scale`, `commit_shrink`); `reconsider` now steps up only via `may_step_up`: refused when the up level was measured over the **line** budget this frame (lines only, so a caller that never calls `begin()`/`note_lines` is unaffected and the existing ladder tests are untouched); once a ratio is known for the level (`step_ratio`: ring of the last 8 measured `lines(up)/lines(level)`, max) it needs `lines * ratio * 1.1 < budget`, else today's fixed margin; the segment margin stays. `fitting_fallback()` (cheapest level measured under both budgets, only when the last judged pass was over and the level is not pinned), `over_budget_last_pass()`, `last_pass_detail()` (the level the last pass was *encoded* at; `detail()` has already stepped on), `settle_on(level)` (no pin; drops the thrown-away pass's owed shading) and `note_settled_pass(segments, lines, pixels)` (so `over_budget()` / `last_lines()` describe the submitted pass). `stage.rs`: `build_plain_scene` and `build_scene_generic` run at most ONE extra encode at `settle_after_ladder(over_last, fallback, last_pass_detail)` (new pure fn). Tests: `buzz-render/tests/ladder.rs` (5: Full 10x NoModelling -> exactly 2 passes then 1 pass next frame; segment-driven oscillation ends on an over-budget Full, `fitting_fallback() == Some(NoModelling)`; a shrunk frame (Full 2x) climbs back; growth forgets the old scale's measurements; pinned has no fallback), `buzz-app/tests/ladder_settle.rs` (4, the pure helper); `cargo test --release -p buzz-render` all green (lib 105 / 1 ignored, every integration file); buzz-app `plain_stage` 2, `stage_lighting` 53 (8 ignored), `ladder_settle` 4 green (run from the already-built exes: cargo's final link of `buzzanimate.exe` is blocked while the app runs). **Measured, ep1 Scene 4 frame 0, `frame_cost --layers 0` (rim on): full warm encode 1521 -> 53 ms; passes 4 -> 1 (`neutral_ctx` refusals 2120 -> 530, 530 a pass); hide-one deltas before 1033 / 993 / 991 / 970 / 963 ms (sum 4.95 s on a 1.52 s frame: not additive), after <= 3.8 ms (three layers -34..-35 ms: their hide-one encode is ~87 ms, an extra probe pass) against `Layer_1 alone` 128 ms: additive.** The frame now settles on the level that fits instead of probing Full every frame; the rim's own 13 M lines a pass (Full) are untouched. A *segment*-driven oscillation (Full over the segment ceiling, lines fine) is not refused in the ladder (lines-only by design); it is caught by the post-loop fallback. |
| L7 Opus review | **passed** | 2026-10-06 | Re-ran `cargo test --release -p buzz-render --test ladder --test rig_lag --test buffer_scale_adaptive`: 5 + 12 + 5 passed. Re-measured `--layers 0` (rebuilt probe): rim on **1468 → 87.7 ms** (the ladder now settles once on the level that fits; the rim's over-budget Full pass is never the one submitted), no rim 91.2 ms, no rim + Full pinned 91.3 ms with **additive** hide-one deltas (Lalten 20, Guddu 16, Munna 14, Raju 14, scenery 2–5). Pipelined 0–120 no rim: render p50 46.5 / p95 48.1 ms, **94 of 121 shown (was 82), 27 dropped (was 39)**; still ~46 ms a frame because the lit `Flat` pass costs ~40 ms → L3′. Accepted the three additions (`last_pass_detail`, `note_settled_pass`, `settle_on` discarding the frame's lighting misses). |
| L3″ stamps under position-independent light + flat sky in set-ups | **done** (Sonnet implementer; Opus review pending) | 2026-10-06 | `LightRig::is_position_independent()` (active, no enabled Lamp, no enabled Sky with `horizon != color`; test `position_independence_is_suns_flat_skies_and_gloom_only` also asserts `field` is identical at five positions). **A symbol holding a bitmap fill is refused under a light** (`SymFlags::images`; found by a test: a lit picture is composited in an isolation group sized to its box in stamp-local space, so a rotated stamp came out 362 pixels / 29 levels off live; unlit pictures still stamp; test `a_symbol_holding_a_picture_is_not_stamped_under_a_light` asserts the refusal and a byte-equal encoding; a lit gradient deviates no more than an unlit one, 87 vs 88 pixels / 4 levels — pre-existing gradient-stamp noise). `try_stamp_symbol`: under `ctx.lighting` it stamps only if the rig is position-independent **and** `ctx.shape_edges == 0.0` (this also makes `draw_gloom_edge`, `crescents` and the lamp branches of `draw_shape` unreachable inside a lit build; a lit **non-similar** placement is refused while a silhouette is gathered: counter `lit_silhouette`); `SymKey` gained `(lit, rig fingerprint, layer depth bits, stage height bits)` (all zero when unlit, so unlit keys are unchanged); the build keeps `lighting` in the child ctx. Silhouette: a lit similarity build records every filled `placed` path per shape (`LightCache::begin_pieces/end_pieces`, `SymEntry/NewStamp.silhouette: Option<Arc<Vec<BezPath>>>`, kept out of any outer figure silhouette while building), and a hit or a build hands `doc * piece` to the figure pass through `add_stamp_silhouette` (per path, so `wound_forward` is decided after placing: a mirrored instance is right). `staging::light()`: the fill sky's horizon = its colour for every setting (`gradient_fill_horizon` keeps the old table, unused); **Night/Interior keys are lamps and the Storm key is a (mixed) sky light, so those settings still draw live; Daylight/Sunset stamp** (test `the_fill_sky_is_flat_in_every_setting`). Counter `stamps_lit` (printed in `frame_cost`'s retention line). Tests `buzz-render/tests/lit_stamps.rs` (6): counters (`neutral_ctx` 0, `stamps_lit` > 0, a held frame builds nothing, a moved rig hits the same entries), refusals (mixed sky / lamp / `Shapes` edges count `neutral_ctx`, build nothing), `SymKey` aliasing (two sun colours = 2x entries, unlit separate), and GPU pixels: sun + flat sky + `Figure` + shadows + rim, a rig of 3 symbol parts (one nests another symbol) + 2 plain instances (scaled/rotated, mirrored), live vs first sighting / build / hit / rig moved / back, 1x and 4x, with and without cull, **within the stamp parity gate** (<= 2 levels, <= 0.1 % of pixels — the gate `headless_symbol_cache` uses: a stamp takes one more f32 rounding than the live walk; the *unlit* stamp path itself differs from live by 5 pixels of 1 level on this very scene; lit differs by 2 pixels of 1 level); refused lights are **byte-identical** to live. Mutation checks: dropping the hit-time silhouette gives 3360 pixels / 92 levels off, dropping the rig fingerprint from the key gives 19099 pixels / 66 levels off. `cargo test --release -p buzz-render -p buzz-light`: all green (lib 105 / 1 ignored, lit_stamps 6, rig_lag 12, ladder 5). **Measured (ep1 Scene 4 frame 0, `--layers 0`, `FRAME_COST_NO_RIM=1 FRAME_COST_FLAT_SKY=1`, quiet machine, L3″ only): Flat 21.1 → 1.99 ms, NoModelling 49.9 → 27.5, Full 52.6 → 30.7; `neutral_ctx` refusals 440 / 530 / 530 → none.** |
| L8′ shadow fragment memo + rig bounds memo | **done** (Sonnet implementer; Opus review pending) | 2026-10-06 | **Shadow fragment** (`ShadowMemo` in `DrawCache`; kill switch `BUZZ_NO_SHADOW_MEMO=1`, `DrawCache::set_shadow_memo(bool)`): `draw_layer`'s shadow pass is now `run_shadow_pass` (the same `push_alpha_group … cast_shadows … pop_isolation`), shared by the live path and by the recording. Signature = `layer_fx_signature` with **no modifier evals and no rim** (the shadow pass draws the stored/tweened objects, never a modifier's copy: each stored object's `Arc` or a fresh rig's pose fingerprint, library `content_id`, animated-symbol playhead) + layer depth + the `ShadowThrow` (its `Debug` form: every field to the bit) + `key.shadow_strength` + `alpha` + the projection's affine bits (not affine: no memo) + cull + the builder's whole view (anchor, scale, `gpu_view`, device scale, clip bounds, visible) + lighting detail. A miss encodes the pass into a child `SceneBuilder` with the frame's own split / clip / device scale (so its bytes are the frame's own) and **appends** it (one encode, not two) and keeps it (an empty pass too); a hit appends the kept fragment and skips `shadow_group_bounds`, `Caster::add` and `Caster::draw` entirely; `lines` charged as stamps do. At most 2 per layer, unused for 2 frames dropped, `Weak`s + library pin the hashed addresses. **Bounds**: `BoundsCache::resolved` keeps its address key but now also (a) drops everything when the library's identity moves (an instance's extent reads its symbol, which an edit changes without touching the instance — it was never invalidated before), (b) for a rig stores and checks `ArmatureData::pose_fingerprint()` (already `pub`, nothing exposed), (c) counts measurements (`computed()`); the shadow pass reads through it for stored objects (`caster_bounds`). **Root cause of the "350 µs per rig" in the profile**: not the rig — `stage.rs:188` clones the scene every frame and `Clone for Scene` hands back an empty `bounds_cache`, so the **first** `resolved_bounds` of every frame rebuilds the whole symbol-bounds table: measured per layer (temporary timers, removed) the first layer's `shadow_group_bounds` is **4.7–4.8 ms every frame** and every other layer's 0.00–0.03 ms; a rig's `pose_fingerprint` is 0.6 µs, `posed()` 1.0 µs, `resolved_bounds` 1.9 µs warm (39 rigid parts). A held frame no longer reaches `Scene::resolved_bounds` at all; a moving frame with owner-less casters still pays the table (fix = keep the cache across `Scene::clone`, `buzz-scene/src/lib.rs`, outside this step). Tests `buzz-render/tests/shadow_memo.rs` (4): bounds (`a_held_rig_is_measured_once_and_a_moved_or_edited_one_again`: counter 1 for two asks, 2 after a re-pose, still 2 held, 3 after a library edit, every answer equal to `Scene::resolved_bounds`), counters (`miss (0,1)` → held `(2,1)` → rig moved `(2,2)` → held `(3,2)` → camera moved `(3,3)` → `(4,3)`, an old view dropped, kill switch `(0,0)`), **`the_shadow_memo_encodes_exactly_the_live_bytes`** (CPU: path / draw / transform / style streams and counts of the memoised frame **byte-equal** to the memo-off frame at miss, hit, rig moved, camera moved, back), GPU pixels memo on vs off at 1x, 1x culled and 4x through held / moved / camera-moved / back with a hit **exactly on the held steps** (within 2 pixels / 1 level: the GPU itself flickers by one level at one pixel in ~1 render of 20 of the *same* encoding on this scene — measured with 60 renders of one encoding — so pixel tests are gated and the byte proof is the CPU one). Mutations: dropping the projection and the view from the signature fails the counter test and gives 1129 pixels / 20 levels off. `cargo test --release -p buzz-render -p buzz-light -p buzz-act` all green; buzz-app (linked fresh, the app was not running) `stage_lighting` 53 (8 ignored), `plain_stage` 2, `ladder_settle` 4; buzz-export `symbol_stamp_scale` 1 and `small_artwork` 4 pass, **`headless_symbol_cache::a_looping_graphic_at_several_phases` fails ("cache never stamped anything") — not from this change (unlit path untouched): its two instances have different inner frames, so under the first-sighting rule (`first_sightings`, uncommitted earlier work, absent from HEAD) neither builds**. **Measured, ep1 Scene 4 frame 0, `--layers 0`, `FRAME_COST_NO_RIM=1 FRAME_COST_FLAT_SKY=1`, quiet (L3″ + L8′), held frame: Flat 21.1 → 2.1 ms (2.0–2.8 over four runs), NoModelling 49.9 → 3.2, Full 52.6 → 6.6; `NoModelling − Flat` 28.8 → 1.1 ms; `neutral_ctx` 440 / 530 / 530 → none.** On the document **as built** (mixed sky, still refused): Flat 21.5 → 20.5, NoModelling 48.9 → 26.8, Full 52.7–59.4 → 29.5–31.5 (the shadow memo alone). **Playback does not move**: pipelined `a` 0–120 (`FRAME_COST_NO_RIM=1 FRAME_COST_FLAT_SKY=1`) render p50 **46.21 → 46.27 ms**, p95 48.16 → 47.64, shown 94 → 94 of 121, dropped 27 → 27. Why (headless 120-frame loop through `stage::build_scene`, ladder not pinned vs pinned): pinned **Flat** playback encode p50 **6.9 ms** (p95 7.6), pinned **NoModelling 27 ms** (5 shadow-memo misses a frame: five moving rigs, ~21 ms of Caster), **Full ~560 ms** here (5 crescent booleans a frame built inline; the app defers them) — so — inferred, the pipelined run does not print its rung — the budget-driven ladder, now that Flat is nearly free, spends it on rungs whose cost is per *moving pose* (shadow caster, figure silhouette + crescents) and none of that is memoisable per pose. The lever left for playback is the shadow caster / bands per moving pose (L8 as first written, per part) or a time-aware ladder (L7), not another cache of a held frame. |
| L3″ + L8′ Opus review (wrap-up) | **accepted; two honest limits recorded** | 2026-10-06 | Implementer's quiet-machine numbers on ep1 Scene 4 f0 with no rim and a flat sky: Flat **21.1 → 2.1 ms**, NoModelling **49.9 → 3.2**, Full **52.6 → 6.6**; refusals 440–530 → 0. Exactness: lit stamps sit inside the existing stamp parity gate (≤ 2 levels on ≤ 0.1 % px — stamps were never byte-identical to live, lit or unlit); the shadow memo is byte-identical on the CPU encoding. Added `SymFlags::images` (pictures never stamp under light), flat sky in staging (Daylight/Sunset get stamps; Night/Interior lamps and Storm still draw live). **Limits:** (1) a document whose sky still mixes by height (the user's open ep1.buzz) stays on the live path — Full ~30 ms there; re-stage or set the sky horizon = colour; (2) playback of *moving* rigs is unchanged at render p50 ~46 ms: the ladder picks NoModelling/Full and their per-pose costs (one shadow-memo miss per moving rig, figure crescent booleans ~100 ms each in a 120-frame loop) are not memoisable → **L5 (figure silhouette from part outlines / cheaper per-pose bands) is the next step.** Also found: `stage.rs:188` clones the scene per frame and `Scene::clone` empties the bounds cache (4.7 ms/frame rebuild) — fix in buzz-scene next session. One pre-existing failure outside this work: `buzz-export headless_symbol_cache::a_looping_graphic_at_several_phases` (first-sighting rule, uncommitted, not from today). |
| L3 stamps under uniform light | **dropped** (encode is 3 ms without stamps; not exact under a height-mixed sky) | 2026-10-06 | table in "Where the no-rim frame goes" |
| L3′ profile (step P) | **done — it falsifies the premise of L3′** (Sonnet implementer) | 2026-10-06 | Temporary timers (removed; `document.rs` is byte-identical to before), ep1 Scene 4 frame 0, `--layers 0`, no rim, `Flat`, machine quiet: warm encode **22.3 ms**, of which `draw_shape` 17.5 ms over 2517 shapes: `LightRig::field` **0.55 ms**, `Illumination::apply`/`map_colors` **0.11 ms**, `ctx.paint` **0.07 ms** — **0.7 ms of lighting arithmetic in all**. The rest is the live encode: shape head (clone + transform + project + bbox) 3.4–3.7 ms, sealed fill encode 10.5 ms over 1268 fills (8 µs each), stroke encode 1.4 ms over 1249, remaining ~4.8 ms of the frame outside `draw_shape` (symbol/layer walk). Full table under "Profile" below. |
| L3′ lit colour path made cheap, bit-identical | **blocked, nothing implemented** — needs a new plan | 2026-10-06 | Items 1–4 together can save at most ~0.7 ms of 22 ms; the acceptance (`Flat` ≤ 10 ms) is out of reach by this route. The "37 ms of lighting arithmetic" was the live encode of ~2500 shapes (plus up to 2× CPU noise from concurrent builds: the same exe measured 37.9 / 86.2 / 92.5 ms for Flat / NoModelling / Full while another `rustc` ran and 21.7 / 51.6 / 57.8 ms on a quiet machine). |
| L8 shadow caster memo per part | **blocked, nothing implemented** — needs a new plan | 2026-10-06 | Quiet machine, `NoModelling` 53 ms (Flat 22): the extra ~31 ms is `shadow_group_bounds` **5.2 ms** (the first `resolved_bounds` per object, ~350 µs each: not a lookup; the draw loop's second call is 0.04 ms), `Caster::add` **7.6 ms** (placement `shadow * breathe(doc * path)` 2.5, `project` 0.95, `wound_forward` 3.6), the object walk **0.4 ms**, `Caster::draw` **13.4 ms** (one giant fill + strokes encoded per caster), plus ~680 more live shapes. A memo of caster inputs at the identity saves only the 0.4 ms walk; a per-pose thrown-and-wound memo saves at most the 7.6 ms; 13.4 + 5.2 ms remain, so ≤ 10 ms is unreachable by this route too. |
| L4 idle re-encode of frame 0 in the real app | not started (needs the `BUZZ_FRAME_LOG` run) | – | – |
| L5 figure silhouette from part outlines | not started (measure after L1/L2) | – | – |
| L6 acceptance (wrap-up state) | **partial — 10–20× better, not yet 24 fps on moving rigs** | 2026-10-06 | Rebuilt app (`target/release/buzzanimate.exe`, 16:45) on the regenerated `episode_v2/ep1.buzz` (no rim, flat sky, bands off), `BUZZ_FRAME_LOG=1 BUZZ_AUTOPLAY=20`: **18.9 fps shown** over 20 s, encode p50 **45.7 ms** / p95 49.8 / max 100, total p50 51 ms, **2.4 M lines a frame (was 13.2 M)**, one slow frame (frame 0 cold, 275 ms). This morning the same scene was 430–1070 ms a frame. Headless: held frame 6.1 ms, zero stamp refusals; moving-rig playback p50 ~47 ms. **Open:** the ~45 ms per moving pose (stamps should hit under rotation — profile what misses: pose-dependent keys, bounds table rebuilt per `Scene::clone`, shadow-memo misses per moving rig); L5 cheap bands; Bimari5 regression run and the full parity gate not re-run today (the stamp parity tests in buzz-render pass). |

## Diagnosis (2026-10-06)

### Real app (release, `%LOCALAPPDATA%/BuzzAnimate/logs/last.log`, run of 10:45)

- `slow frame` lines: **encode 430–1070 ms per frame**, present 10–12 ms,
  `stage_bands 0`, `lights scale 8`. So it is CPU encode, not GPU, not bands.
- Frame 0 re-encoded **15 times in 13 s at ~450 ms each** while nothing
  played (frames `0` repeated), then frames 1431–1472 at 500–800 ms during
  playback. The status bar's ms readout is therefore honest here: the frame
  really costs that.

### Real app, `BUZZ_FRAME_LOG=1`, a copy of ep1.buzz opened idle for 30 s (11:16)

- Four frames of frame 0 after the open: encode 885 / 456 / 465 / 448 ms,
  `why splash,shade_build,egui_repaint … stale 1`, then one 0 ms frame and
  **silence**: an untouched window does settle. **Lines per frame:
  13,199,029** (Bimari5's heaviest frames were 5–7.7 M). So the 15
  re-encodes of frame 0 in the user's own log were gestures or edits moving
  the stage stamp (revision/camera), each paying a full 450 ms encode: that
  is the lag they feel while working on frame 0, and the playback numbers
  are the same cost per frame.

### Headless (`frame_cost ep1.buzz --layers 0`, Scene 4, 1600×900, warm)

- Full warm encode **1519 ms**. Hiding any one of the four character layers
  saves 950–1160 ms each, and hiding the scenery `Layer_1` layers 570–950 ms
  each: the deltas sum to far more than the total, so the cost is not a sum
  of per-layer work — it is superlinear in what is on stage.
- `Guddu` alone: **138 ms warm** for one rig.
- Stamp refusals: `neutral_ctx = 2120` per full frame (90 for Guddu alone):
  every symbol instance on a lit layer is refused by the stamp cache.

#### Lighting ladder (`FRAME_COST_DETAIL`, `--layers 0`, same frame, warm)

| Rung | What it keeps | Full warm encode | One rig alone |
|---|---|---|---|
| Full (what the app runs) | tint, pool, cast shadows, terminator + highlight | 760 ms (1468 ms in the earlier run) | Lalten 193 ms |
| NoModelling | tint, pool, cast shadows | 87 ms | – |
| Flat | tint and pool only | 39 ms | Lalten 11 ms |
| unlit | – | 3.5 ms | 0.4 ms |

Edge modes at Full detail: `Figure` (the default) 760–1468 ms, `Shapes`
1661 ms, `Off` 1557 ms (one rig alone 688 ms; an earlier run gave 5131 ms —
the numbers swing run to run, the ranking does not). `Off` + `Flat`: 74 ms. `Off` being the slowest proves
the bands are not the bulk of "modelling": what `LightDetail::Full` turns on
and `NoModelling` turns off, in *every* edge mode, is the **rim glow**
(below, C0). So: rim ≈ most of the 670 ms, bands next, cast shadows ~6 %,
tint with its stamp refusals ~5 %. Fix order: L0 (rim/silhouette) and
L1/L2/L5 (bands) first, then the shadow caster (a per-part memo once L1
gives parts an identity), then L3.

## Root causes, in the code

| # | Where | What |
|---|---|---|
| C0 | `document.rs` ~3363–3444 (`draw_layer`: `rim`, `needs_silhouette`, `layer_fx`) | **The key light's rim glow is rebuilt every frame for every lit layer, uncached**: `append_silhouette` walks every posed shape of every object on the layer (plus `outline_stroke` at 0.1 tolerance for each stroked shape), and `buzz_fx::build` lays a Glow filter over that thousands-of-subpaths path. Nothing keys it on the layer's content, so a layer whose rig has not moved pays it again. Runs in every `EdgeMode` whenever `detail().models()` and the key light has `rim > 0`: a plain `Light` has `rim: 0.0`, but every set-up scene gets one (`buzz-act/src/staging.rs` ~822 `key.rim = …`, "the rim is the point of lighting a set-up scene"), so every episode scene pays it. `figure_edges()` is 0 under `EdgeMode::Off`, so the 1557 ms `Off` frame is the rim (plus tint and shadows) with no bands at all. |
| C1 | `buzz-render/src/document.rs` ~4861 (`ObjectKind::Armature`) | Posed parts are drawn with `owner = None`. In `lighting.rs` ~639 `LightCache::geometry` returns `build_geometry(..)` for an owner-less shape: **the two crescent booleans are rebuilt for every shape of every rig every frame and never cached.** The figure pass (~4347) likewise gathers the silhouette every frame when the owner is `None`. |
| C2 | `document.rs` ~4479 `figure_content` | For an object that contains instances (every slimmed rig: `episode::slim_rig` makes each rigid part an instance) the hash XORs `ctx.elapsed`, so **the figure's silhouette union and crescents are rebuilt every frame even when the pose has not changed.** The bone angles are not in the hash at all. |
| C3 | `document.rs` ~5377 `try_stamp_symbol` | `ctx.lighting.is_some()` refuses every stamp. On a lit layer a 1400-shape part is re-encoded live every frame: the 2120 refusals. |
| C4 | `buzz-scene/src/rig.rs` ~157 `ArmatureData::posed` | Allocates a fresh `Arc<Object>` per part per call, and is called from the renderer, `Object::flatten` (culling, hit-testing), `armature_resolved_bounds` and `posed_paths`: the deformation runs several times per frame and nothing downstream can key a cache on the result. |
| C5 | `buzz-app/src/app.rs` ~11912 reuse decision | The idle frame-0 re-encode. Not yet explained: owner-less shading never queues a miss, so `stage_stale` should not come from the rigs. Candidates: deferred misses from the scenery's lit shapes (inline budget exhausted every frame), `lights_generation` moving as batches land, or a repaint source. **Decided by the `BUZZ_FRAME_LOG=1` run (L4), not by guessing.** |

## What the L0-L2 measurement showed (2026-10-06, after the three steps)

Temporary timers inside `draw_layer` (removed again) on ep1 Scene 4 frame 0,
`frame_cost --layers 0`:

| Phase of one warm `build_scene` (2 Full passes of 4) | ms | estimated lines |
|---|---|---|
| `draw_ops(&fx.behind)` (the rim glow ops, 10 layers) | **1144** | **26.3 M** (13.2 M per pass) |
| `FigurePass::close` (figure bands) | 8 (was **1117** on the cold first frame, before L1/L2 warmed it) | 0.26 M |
| layer-fx gather + `buzz_fx::build` on a miss (cold) | 2-6 + 5-8 per rig layer (silhouettes are 48-78 k path elements) | – |
| total | 1474 | – |

1. **L1/L2 work**: after the first frame every figure is a cache hit (`light
   builds 12, places 12`, then 0 new), `FigurePass::close` is 8 ms, the
   layer-fx memo hits 10 of 10 layers on every later pass.
2. **C0 was right about the rim and wrong about where it costs.** Gathering the
   silhouette and building the glow is ~15 ms a layer. What costs ~570 ms a
   pass is **drawing the glow ops**: `crate::filters::draw_ops` re-flattens and
   re-encodes the stroked bands over a 50-78 k-element silhouette for every
   rig layer, **13 M lines a pass against 1.4 M for everything else in the
   frame**. A memo of the ops cannot help; only encoding fewer lines, or
   reusing the *encoded* vello fragment (same projection and tolerance), can.
3. **The re-encode ladder oscillates on this frame, which doubles the bill.**
   `reconsider` at scale 8 (the max): Full = 14.69 M lines > budget 14.4 M ->
   steps down to NoModelling (1.39 M lines, fits) -> `room_to_step_up` judges
   that Full fits (its margins assume modelling is ~2.5x the rest; the rim made
   it 10x) -> steps up again -> `build_scene`'s `for _ in 0..4` runs
   Full, NoModelling, Full, NoModelling and **stops on a Full (over-budget)
   pass**. That is why hiding any one layer saves ~930 ms (the frame then
   fits at Full and runs one pass), why the deltas were not additive, and with
   `FRAME_COST_DETAIL=full` pinned (one pass) the warm encode is **670 ms**
   with additive per-layer deltas (Layer_1 249, Lalten 165, Munna 138,
   Guddu 134, Raju 113). It is also a candidate for the real app's repeated
   frame-0 re-encodes (L4) and for frames that "vanish" (a Full pass over the
   line budget being the last one encoded).
4. So the next step is not another cache of the same thing: either cut the
   lines the rim emits (exactly, or with a stated tolerance), or encode the
   rim once per `(layer signature, projection)` as a vello fragment and
   `append` it, and fix `room_to_step_up` so it cannot step up into a level
   the line budget has just refused.

## Fixes (all exact: the same pixels as today's live draw)

### L0 — the layer silhouette and its rim, cached and cheap (buzz-render)

1. **Cache per layer.** `DrawCache` gets a `silhouettes` memo: key =
   (layer id, hash over the layer's resolved objects of: `Arc` address,
   `prepend`/modifier evaluation, breath, and — for an armature — its pose
   (bone angles) and part transforms; `elapsed` only for an instance whose
   symbol has `length() > 1`), value = `Arc<BezPath>` silhouette and the
   `buzz_fx::build` result for the rim+filters (keyed additionally on the
   rim's reach/colour and the filter list). A hit skips both the gather and
   the build: exact, since the same path and filters give the same ops.
   Age entries out like the shading cache does (unused for N frames).
2. **Cheap on a miss, for rigs.** For an `ObjectKind::Armature` the gather
   uses each **part's outline** instead of each shape: the outline of a part
   is `append_silhouette` of that part's artwork at the identity, computed
   once per `part.artwork` `Arc` (memo in `DrawCache`, keyed on the address)
   and placed by the posed transform. Exact: concatenation under non-zero is
   order- and grouping-independent, so the union of per-part concatenations
   is the same silhouette as the flat walk. (Skinned parts have no fixed
   outline and keep the flat walk.)
3. Test: a lit layer with one rig drawn on two frames with the same pose
   gathers once (count `append_silhouette` shape visits via a debug counter)
   and the rim ops are pointer-equal; a changed pose rebuilds; the rendered
   pixels with the memo on and off are identical (render test at 1×, 4×).

### L1 — stable posed parts, with an identity (buzz-scene + buzz-render)

- `ArmatureData` gets a `posed: OnceLock<Vec<Arc<Object>>>` memo (skipped by
  `Clone`, `PartialEq`, serde — implement `Clone` by hand or wrap the memo in
  a type whose `Clone` yields an empty cell). An armature object stored in a
  keyframe is immutable (copy-on-write everywhere), so its posed parts are a
  pure function of it; tweened poses are new objects and get their own memo.
  `posed()` returns the memo (one allocation per object lifetime).
- Renderer: `draw_object(builder, &part, Some(&part), …)` for posed parts.
  The `LightCache` key is `(Arc address, shape index, quantised doc)`, so a
  held pose hits every frame and a moved bone misses exactly as a moved
  object does. Hit-testing and bounds get the same `Arc`s for free.
- Skinned parts (`RigBinding::Skin`) are included: the memo makes their
  deformed path stable per object too.
- Tests: `posed()` twice gives pointer-equal `Arc`s; cloning the armature
  does not share the memo; a changed angle (new object) gives new parts;
  `buzz-render` lighting test: drawing a lit rig twice builds crescents once
  (`LightCache` places count stable, `inline_spent` ~0 on the second frame).

### L2 — figure content from the pose (buzz-render)

`figure_content` for `ObjectKind::Armature`: hash every bone angle and every
part's `artwork` address/transform, plus the library `content_id`; add
`elapsed` only when some part instance's symbol has `length() > 1` (its
silhouette really depends on the inner frame). Same rule for a `Group` of
instances. Test: a lit rig drawn on two consecutive frames with the same
pose reuses the figure geometry (`LightCache::ready` hits); a different pose
misses.

### L7 — the detail ladder must not step up into a level it just refused (buzz-render)

`DrawCache::reconsider` (document.rs ~698) steps **down** when the pass is over
budget and steps **up** when the pass is under `room_to_step_up`, a fixed
margin per level (`Flat` ⅓, `NoModelling` ⅙ of the budget) that encodes an
assumption about how much the next level adds (shadows ×1.5, modelling
×2.5). On a frame where the next level adds ×10 (the rim) the two rules
contradict each other and `build_plain_scene`'s four passes go Full →
NoModelling → Full → NoModelling, ending on whichever pass is fourth — an
over-budget Full here. Fix, exact (only *which* level is chosen changes,
and that already varies frame to frame):

1. `DrawCache` remembers, for the current frame (reset in `begin()`), the
   lines each `(scale, detail)` pass actually produced (`measured:
   [Option<u64>; 4]` per scale). A step **up** to level `L` is refused when
   `measured[L]` at this scale is `Some(lines)` with `lines > budget`.
2. Across frames, keep the last measured ratio `lines(L.up()) / lines(L)`
   for each level (decayed, say the max of the last 8 frames) and step up
   only when `self.lines × ratio × 1.1 < budget`; fall back to the fixed
   margin while no ratio has been measured.
3. `build_plain_scene` / `build_scene` must never *end* on a pass that was
   judged over budget when a fitting lower pass exists: if the fourth pass
   is over budget, re-encode once at the last fitting level (bounded: at
   most one extra pass).
4. Tests (buzz-render, no GPU): a synthetic `DrawCache` ladder where Full
   measures 10× NoModelling and NoModelling fits: the sequence is Full →
   NoModelling → stop (2 passes), never oscillates; a frame that later
   shrinks (Full now 2×) steps up again; the existing `reconsider` tests
   stay green. `frame_cost ep1.buzz --layers 0` with rim on: additive
   hide-one deltas (each layer's delta ≤ its own `alone` cost + 10 %).

### Where the no-rim frame goes (measured 2026-10-06, `--layers 0`, one pass pinned)

| Setup | Full warm encode |
|---|---|
| unlit, stamp cache **off** (`BUZZ_NO_SYMBOL_CACHE=1`) | **2.95 ms** |
| unlit, stamps on | 3.71 ms |
| lit, no rim, `Flat` (tint only) | 40.6 ms |
| lit, no rim, `NoModelling` (+ cast shadows) | 85.0 ms |
| lit, no rim, `Full` (+ figure bands) | 96.5 ms |

So encoding every path of the frame live costs **3 ms**; stamps are not
what the lit frame is missing. The lit `Flat` pass adds **~37 ms of
per-shape lighting arithmetic** (`LightRig::field` per shape, `to_linear`
conversions, `Illumination::apply`/`map_colors`, paint rebuilds), the
shadow pass **~44 ms**, the bands **~11 ms**. L3 as first written (stamps
under light) is therefore dropped — it could not be exact under a sky that
mixes by height anyway — and replaced by L3′ and L8 below.

### L3′ — the lit colour path made cheap, bit-identical (buzz-light + buzz-render)

1. **Profile first** (temporary timers, removed after): split the ~37 ms of
   the lit `Flat` pass between `field()`, `apply`/`map_colors`,
   `ctx.paint()`/paint allocation, and the seal/shape bookkeeping, on
   ep1 Scene 4 frame 0 (`FRAME_COST_NO_RIM=1 FRAME_COST_DETAIL=flat`).
2. Then fix the top items, each **bit-identical at u8** to today's output:
   - `LightRig::field` for a rig of suns and skies depends on position only
     through the sky's `y / stage_height` mix: build once per frame (per
     light-rig fingerprint) a table of the ambient term against `y` at a
     resolution finer than 1/255 of the mix (e.g. 4096 rows) and look it
     up; lamps/gloom keep the full computation.
   - `to_linear` / `from_linear`: 256-entry LUT for the u8 → linear step
     (exact: the input is u8), and a from-linear path whose u8 result is
     proven equal to the current formula for every u8 input × a grid of
     light levels (exhaustive test over 256 inputs × 64 light levels per
     channel).
   - `Paint::map_colors` on `Solid` without allocating; cache the lit paint
     per `(shape owner address, index, field fingerprint)` for the stable
     owners L1 gave rig parts, so a held frame does no colour math at all.
3. Acceptance: lit `Flat` pass ≤ 10 ms on that frame; a lit render before
   and after is **byte-identical** (test at 1× and 4× on a synthetic sun +
   sky + lamp scene, and on ep1 frame 0 via `frame_cost`'s pixel probe).

### L8 — the shadow caster (buzz-render)

`NoModelling − Flat = 44 ms` for the cast shadows of four rigs and the
scenery. Profile `draw_layer`'s shadow pass (`shadow_group_bounds`,
`push_alpha_group`, `cast_shadows_within`, `Caster::add` → `project`,
`wound_forward`, `outline_stroke` for stroked shapes, `Caster::draw`).
Then: memoise per rigid part (stable `Arc`, L1) its caster paths at the
identity (fills and the stroke outlines, exactly what `Caster::add` pushes
before the throw), and per frame only `throw × pose × path` +
`wound_forward`; skip `outline_stroke` for parts without strokes. Exact by
construction (same paths). Acceptance: shadow pass ≤ 10 ms on that frame,
pixels byte-identical.

### Profile (step P of L3′ / L8, 2026-10-06) — the premise of both is wrong

Method: temporary `Instant` guards (a static counter per call site, dumped in
`DrawCache::begin`; all removed afterwards) in `draw_shape`, the shadow pass and
`Caster`; `frame_cost ep1.buzz --layers 0`, `FRAME_COST_NO_RIM=1`, one pass
pinned, warm passes only. **Run on a quiet machine**: while another `rustc` is
running the same exe is up to 2× slower (Flat 37.9 vs 21.7 ms), so every number
below is from a run with no compiler alive.

| `Flat`, lit, no rim (warm encode 22.3 ms) | ms | calls |
|---|---|---|
| `draw_shape` total | 17.5 | 2517 |
| · `LightRig::field` | 0.55 | 2517 |
| · `Illumination::apply` / `map_colors` (fill) | 0.11 | 1268 |
| · `DrawCtx::paint` | 0.07 | 1268 |
| · head: `doc * path`, `project`, `bounding_box` | 3.4–3.7 | 2517 |
| · fill encode, sealed (`fill_shape_paint_sealed_sized`) | 10.5 | 1268 |
| · stroke: apply + paint + encode | 1.4 | 1249 |
| outside `draw_shape` (symbol / layer walk, resolution) | ~4.8 | – |

| `NoModelling` adds (warm encode 53 ms) | ms | calls |
|---|---|---|
| `shadow_group_bounds` (first `resolved_bounds` of each object) | 5.2 | 10 |
| `Caster::add` | 7.6 | 3199 shapes |
| · `shadow * breathe(doc * path)` | 2.5 | 3199 |
| · `project` | 0.95 | 3199 |
| · `wound_forward` | 3.6 | 1578 |
| `cast_shadows_within` object walk (total − add − draw) | 0.4 | 15 |
| `Caster::draw` (one fill of the whole caster + strokes, vello encode) | 13.4 | 15 |
| draw loop's second `resolved_bounds` | 0.04 | 15 |
| `draw_shape` (more live shapes than Flat: 3199 vs 2517) | 21.3 | 3199 |

What it means. (1) The lighting arithmetic is **0.7 ms of 22**; the LUT, the
sky table, the `(illumination, colour)` cache and the allocation-free
`map_colors` of L3′ can win only that. (2) The unlit frame is 2–3 ms **because
it is a cache hit** (warm unlit passes draw ~10 shapes; the cold ones draw
~2400 in 15–30 ms, the same as lit), not because live encoding is cheap — the
plan's "encoding every path of the frame live costs 3 ms" was a misreading.
What lighting really costs is that a lit layer cannot use the layer / symbol
stamp caches (440–530 `neutral_ctx` refusals), so ~2500–3200 shapes are
encoded every frame at ~7 µs each. (3) The shadow pass is dominated by
`Caster::draw` (13.4) and `shadow_group_bounds` (5.2), neither of which an
identity-space memo of the caster inputs touches; the walk it would save is
0.4 ms. Candidate levers for the next plan, not decided here: a lit layer's
encoding kept as an appendable fragment keyed on the light rig and the layer's
content (what the stamp cache does for unlit layers); a cheaper
`resolved_bounds` for a held rig (5.2 ms); the caster's fill kept as an
appendable fragment per held pose and light.

### L3″ — stamps under position-independent light (buzz-render + buzz-act)

The profile above settles it: a lit layer's cost is the ~2500–3200 shapes it
encodes live because `try_stamp_symbol` refuses every instance when
`ctx.lighting` is set (C3). The light itself is 0.7 ms. So the stamp cache
has to work under light — **exactly**.

Why it can be exact: with a rig of suns and a *flat* sky (`Sky { horizon }`
equal to the sky's colour), `LightRig::field` does not depend on position
at all (a sun's `direct_at` is constant; the sky's `ambient_at` mixes only
when `horizon != color`; the base is constant). Then every shape of a
symbol gets the same `Illumination` wherever the instance stands, and a
stamp encoded *with that illumination applied* is the live encoding, bit
for bit, for every instance under the same rig.

1. `LightRig::is_position_independent(&self) -> bool`: active, no enabled
   `Lamp`, no enabled `Sky` whose `horizon != color`, (gloom lights are not
   directional and do not enter `field`; the figure pass handles them).
   `LightRig::fingerprint()` already exists.
2. `try_stamp_symbol`: when `ctx.lighting.is_some()`, refuse unless
   `ctx.lights.is_position_independent()` **and** `ctx.shape_edges == 0.0`
   (no per-shape crescents inside the stamp — `EdgeMode::Figure` or `Off`;
   `Shapes` mode still draws live) **and** `ctx.lights.key()`'s shadows do
   not depend on anything per shape (they are a separate pass — unaffected).
   Extend the stamp key (`SymKey`) with `(rig fingerprint, layer_depth bits,
   stage_height bits, lit: bool)`, so an unlit stamp and lit stamps under
   different rigs never alias. The build walks `draw_symbol_contents` with
   the *same* `inner_ctx` (lighting kept), so `draw_shape` lights each shape
   exactly as live; the geometry is encoded in the stamp's local space as
   today. **Check** how the build sets `doc` for `draw_shape`'s `placed`:
   the `field(region)` call must not care (it does not, under 1.), and
   `brush_to_doc`/`lamp_lit` must not be reached (no lamps under 1.).
3. **Figure silhouette on a hit.** A figure pass gathering its silhouette
   (`cache.lights.collecting()`) needs every stamped symbol's fills: at build
   time record the symbol's silhouette (the `placed` fills in stamp-local
   space, exactly what `add_silhouette` would have received) in the stamp
   entry; on a hit, `add_silhouette(&(doc * stored))`. Otherwise the bands
   would miss the stamped parts — test it.
4. `buzz-act/src/staging.rs` `light()`: the fill sky's `horizon` = the fill
   colour (flat sky) for every setting, with a comment; keep `fill_horizon`
   values in the table as the documented alternative; the Lights panel can
   still set a horizon (then that document draws live, as today). Update the
   staging test that asserts on the sky if any.
5. Tests: (a) buzz-render GPU pixel test — a lit scene (sun + flat sky,
   Figure edges, shadows on) with a rig of three symbol parts drawn with
   stamps on vs `set_symbol_reuse(false)`: byte-identical at 1× and 4×, on
   two frames with the rig moved between them; (b) the same scene with a
   mixed sky (`horizon != color`) refuses the stamps (refusal counter) and
   still renders identically to live; (c) a lamp refuses; (d) the figure
   bands with stamps on equal the bands with stamps off (covered by (a) if
   the rig is lit as a figure); (e) `SymKey` aliasing: the same symbol under
   two rigs gives two entries.
6. Measure (quiet machine, `scratchpad/l3/quiet_run.sh`): `--layers 0`
   `FRAME_COST_NO_RIM=1` at `Flat`, `NoModelling`, `Full` before/after; the
   refusal counter should fall from ~440–530 to ~0 for the rig parts; target
   `Flat` ≤ 8 ms (from 21.7).

### L8′ — the shadow pass: appendable fragment and cheap bounds (buzz-render)

Profile: `Caster::draw` 13.4 ms (vello-encoding one 3199-subpath fill plus
strokes, every frame), `shadow_group_bounds` 5.2 ms (the first
`resolved_bounds` of each object: ~0.5 ms per rig), `Caster::add` 7.6 ms.

1. **Bounds memo**: `BoundsCache::resolved` for an `ObjectKind::Armature`
   keyed on the object `Arc` + the pose fingerprint (L1's), so a held rig
   answers from the memo (≈ 0 ms); a moved rig recomputes once.
2. **Shadow fragment per layer**: key = (layer id, L0's content signature of
   the layer's objects incl. pose, `shadow` affine bits, `key.shadow_strength`,
   the projection's affine bits — a camera move invalidates); value = the
   encoded vello fragment of the whole shadow group (what `push_alpha_group`
   … `pop_isolation` produced), appended on a hit. Exact: the same inputs
   give the same encoding. Aged like the L0 memo; kill switch
   `BUZZ_NO_SHADOW_MEMO=1`. On a miss, `Caster` runs as today.
3. Tests: pixel identity memo on/off (shadows on, two frames: held then
   moved, camera moved on a third), refusal/hit counters, byte-equal.
4. Measure: `NoModelling − Flat` from ~30 ms (quiet) to ≤ 5 ms when held.

### L3 — stamps under uniform light (buzz-render, parity-gated) — DROPPED

Superseded by L3′ (see the table above): the live encode is 3 ms, and a
stamp baked with one illumination is not exact under a height-mixed sky.

When `ctx.lighting` is set but the light **field over the symbol's bounds
is uniform** (`field.disc().is_none()`: suns/skies, no lamp falloff) and no
per-shape edges are drawn inside it (`shape_edges == 0`, i.e. `EdgeMode::Off`
or `Figure`), stamp the symbol **unlit** and lay the light over the stamp
the way `draw_lit_composited` does for bitmaps: one isolation layer, the
stamp, then the light as a `SrcAtop` multiply (up to full) / screen (above
full). `Illumination::apply` is that arithmetic per colour, so the body
colours land the same; the figure pass still draws the bands over the
figure. Cast shadows and the rim keep their current path (they are drawn
from the silhouette, not per shape).

**Gate:** `zz_zoom_probe`-style pixel diff, cache on vs off, on ep1 Scene 4
frames 0/60/120 at 1×/4×/16× and on a lit copy of Bimari5 frame 20: ≤ 2/255
on ≤ 0.1 % of pixels. If the gate fails because of the sRGB-vs-linear glint
difference the `draw_lit_composited` doc describes, L3 ships **off**
(`BUZZ_LIT_STAMPS=1` opt-in) and the review says so; the user decides.

### L4 — the idle re-encode (buzz-app)

Run the release exe on `ep1.buzz` with `BUZZ_FRAME_LOG=1` for 20 s idle,
then `BUZZ_AUTOPLAY=20`. Read each frame's `why … wakes …` tail. Fix the
one cause found (expected: a `stage_stale` loop from deferred lighting misses
that never install because the frame budget is spent before they run, or a
repaint request). Acceptance: idle open scene → 0–3 redraws/s and **0**
re-encodes of an unchanged frame.

### L5 — figure silhouette from part outlines (measure first)

After L1/L2, time the silhouette gather + union for one moving rig (a
walking frame). If > 5 ms: gather the silhouette from each part's outer
outline (its hidden-completion base fill from T3 of the trace plan) instead
of every shape. Exact for the silhouette whenever every visible shape lies
inside its part's base fill, which T3 guarantees; assert it in debug builds.

### L6 — acceptance

- `frame_cost ep1.buzz --layers 0`: full warm encode ≤ 25 ms.
- `FRAME_COST_FRAMES=0-300 frame_cost ep1.buzz a`: encode p95 ≤ 25 ms, GPU
  ≤ 20 ms, no frame > 40 ms after its first appearance.
- Real app (`BUZZ_FRAME_LOG=1 BUZZ_AUTOPLAY=30`): ≥ 23 fps shown, 0 gaps
  > 100 ms, idle 0–3 redraws/s.
- Bimari5 `frame_cost a` p50/p95 within 10 % of the 2026-10-01 numbers
  (p50 5.7 / p95 79.6 ms pipelined).
- Parity: existing stamp parity tests + the L3 gate; `cargo test --release
  -p buzz-render -p buzz-scene`, `-p buzz-app --test stage_lighting
  --test plain_stage --test seek_ahead --test prebuild`.

## Measurement notes

`frame_cost` now loads `.buzz` (`read_input`: active scene, or
`FRAME_COST_SCENE=<index>`), and takes `FRAME_COST_UNLIT=1` (light rig
disabled) and `FRAME_COST_EDGES_OFF=1` (`EdgeMode::Off`) for the A/B.
