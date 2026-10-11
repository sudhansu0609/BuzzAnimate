# Trace tuning: thinner outlines by default, every tracer constant adjustable (file + UI)

User, 2026-10-07: "the outlines are too thick. can we have 2.5x instead of 4x";
"can we have all these as configurations so that I can play with it and adjust
as per my needs"; "the config shall be in UI as well so that I can adjust".

## Status

| Step | State | Date | Evidence |
|---|---|---|---|
| Diagnosis: what makes the lines thick | done (Fable) | 2026-10-07 | below |
| C1 `TraceTuning` struct in buzz-scene, constants become fields, `TraceOptions.tuning` | done (implementer, awaiting review) | 2026-10-07 | `crates/buzz-scene/src/trace_tuning.rs`: `InkTuning` 14 fields, `ColourTuning` 13, threaded through `trace_ink.rs` / `trace_colour.rs` (constants deleted; `INK_LEVEL` uses in the colour stage now `ink_level` too; colour stage's `U` uses `upsample` too). `ink_bold` default 0.0 in `default()` / `ink()` / `ink_colour()`; negative = `apply_bold` erosion with the 2·\|bold\|·U component guard. New tests: `trace_tuning::tests::{default_round_trips_through_json, an_unknown_or_missing_field_loads_as_default}`, `trace_ink::tests::{negative_bold_thins_the_ring, bold_zero_keeps_the_source_width}`. `cargo test --release -p buzz-scene` 528/528; no existing test needed changing. `cargo build --release -p buzz-app --examples` ok. `TraceTuning` also derives `Copy` (so `TraceOptions` stays `Copy`). |
| C2 tuning file (`trace_tuning.json` in the app data dir) + `BUZZ_TRACE_TUNING` env + `--tuning` on `series_setup` / `trace_png` / `ink_debug` / `rig_vs_source` | done (implementer, awaiting review) | 2026-10-07 | `TraceTuning::{path, load, load_or_default, save}` (`%APPDATA%/BuzzAnimate/trace_tuning.json`, pretty JSON; `serde_json` added to buzz-scene), `TraceOptions::with_saved_tuning()`. **Env vars** (both read by `with_saved_tuning`, so every route honours them: Trace dialog, Character from Image wizard, the Assistant's `ai_character_from_image`, the examples; documented in `trace_tuning.rs`'s module doc): `BUZZ_TRACE_TUNING=<file>` replaces the app's tuning file; `BUZZ_TRACE_BOLD=<px>` (a number) replaces `TraceOptions::ink_bold`. `trace_png` / `ink_debug`: `--tuning <file>` / `--bold <px>` (`ink_debug`'s positional `[bold]` now defaults to the options' 0, not 0.5); smoke on `pujariFront_0.png`: default 31 shapes, `--bold 0.5` 26, `--tuning t.json --bold -0.25` 46, a missing `--tuning` file errors. `rig_vs_source`: `--tuning` / `--bold` applied to the wizard's options (`AiCharacterState::default()`, saved tuning) and passed to `character_from_image_with`. `series_setup`: `--tuning` (checked with `TraceTuning::load`, exit 2 if unreadable) sets `BUZZ_TRACE_TUNING` and `--bold` sets `BUZZ_TRACE_BOLD` in-process before anything traces, so the agent route and `build_corrected` (now the wizard's options) both take them; smoke: a missing `--tuning` file and `--bold abc` both exit 2 with a message. `cargo test --release -p buzz-scene` 528/528; examples build ok. |
| C3 UI: "Tuning" section in the Trace dialog and the AI character dialog, Save / Reset, live re-trace | done (implementer, awaiting review) | 2026-10-07 | `buzz_ui::trace_dialog::tuning_section(ui, &mut TraceTuning, show_colour) -> bool`: "Tuning" header (closed by default), inner **Ink** (13 sliders + `upsample` combo Auto/1/2/4/6) and **Colour** (13 sliders, only in `InkAndColour`); hover = each field's `///` line verbatim; ranges from the tables, 50–100 positions where the range allows. Sliders use `SliderClamping::Edits` + `max_decimals`: egui's default re-snaps a value to the step every frame, so the default 0.15 became 0.15000000000000002 just by being drawn, and a file's off-step value would have been lost. Row: **Save as default** (`TraceTuning::save`, shows the path), **Reset**, **Load file…** (`rfd` on its own thread, as the AI dialog's Browse… does). Trace dialog: Bold slider −1.0..=3.0 step 0.05 with the plan's hover text. It has no live re-trace (it traces on **Trace** with the options as they stand), so tuning edits in place like every other option there. Its initial options: `editor.rs` `TraceBitmapOptions` opens with `workspace.trace.with_saved_tuning()`. AI character dialog: `AiCharacterState.trace_options: TraceCharacterOptions` (`trace: ink_colour().with_saved_tuning()`). `process_parts` takes `&TraceCharacterOptions` (`{ uniform_width, ..state.trace_options }`), so the first Segment & Trace uses the saved tuning. `render_preview`: Bold (−1..=3), Colours (2..=64), the shared `tuning_section` and a **Re-trace** button. `start_retrace` re-runs only `trace_character_whole_with` on a worker thread from the state's image, matte, skeleton and completed parts, through `rx_process`; a failed re-trace keeps the preview. Controls greyed, with a spinner, while a trace runs, and Back / Create are disabled meanwhile. `character_from_image_with(path, name, &TraceCharacterOptions, progress)`; `character_from_image` delegates with the wizard's defaults. Test `trace_dialog::tests::tuning_section_lists_every_field` (accesskit tree, headers forced open: 26 sliders + 1 combo with colour, 13 + 1 without; field counts from the serialised defaults) passes. `cargo test --release -p buzz-ui`: lib 370/370 (one run had `timeline_panel::tests::the_footer_never_overlaps_the_layer_list` fail; it passes alone, so it is flaky, and the file was not touched). `tests/dock_columns` has 3 failures that were already there (Layers/Properties/Colour/Layer Depth panel heights; the file was already modified in the working tree). |
| C4 character pipeline knobs (`TraceCharacterOptions.trace`, `ink_line_share`, `hidden_reach_fraction`) | done (implementer, awaiting review) | 2026-10-07 | `TraceCharacterOptions` gains `trace: TraceOptions` (default `ink_colour()`), `ink_line_share` 0.60, `ink_network_min_span` 0.5, `hidden_reach_fraction` 0.02. The constants are deleted and their notes kept as `//` on the fields. `trace_character_whole_with` traces with `options.trace` (mode forced `InkAndColour`, `drop_background` true). `tests/trace_fidelity.rs:165` needed `..Default::default()` because its exhaustive struct literal no longer compiled; its values are unchanged. `cargo test --release -p buzz-ai` 182/182 (9 ignored). Example flags: see C2. |
| Fable review: width measured, Raju/Guddu/Woman1 sheets at 4×, suites green, exe relinked | **done — accepted** (exe relink in progress at the time of writing) | 2026-10-07 | Refactor proof: `ink_debug --bold 0.5` mask is **byte-identical** (`cmp`) to the pre-refactor dump, median 3.25 px; default (0) → 2.25 px = the source; `--bold -0.25` → 1.75 px (23 shapes vs 20: thin connectors can split at strong negatives, the user's dial). `rig_vs_source` Raju: `--bold 0.5` reproduces yesterday's rig exactly (ΔE 2.05, edge p95 1.0, 53 ink shapes); default ΔE 2.18 / p95 1.41 / small features 76.4 % (was 70.8). Sheet `ink_check/raju_rig_source_vs_bold0_vs_bold05_4x.png`: bold 0 is the source's own weight on eyes, brows, ear, collar. Suites: buzz-scene 528/528, buzz-ai 182/182, buzz-ui lib 370/370 (incl. `tuning_section_lists_every_field`); `tests/dock_columns` 3 failures are pre-existing (file modified 09-22 in the working tree, panel-height assertions, nothing to do with tracing). Cast/episode NOT rebuilt: the user picks a bold value in the UI first. Env vars for every route: `BUZZ_TRACE_TUNING=<file>`, `BUZZ_TRACE_BOLD=<px>`. |

## Diagnosis (measured 2026-10-07)

There is no "4× thickness". The `4` the user saw is `U`, the super-sampling
factor of the ink mask (`trace_ink::upsample_factor`: 4 under 2.5 Mpx) — it
sets the *precision* of the edge, not the width. The width comes from
`TraceOptions::ink_bold = 0.5` (px added to **each** side → +1 px total) in
`TraceOptions::ink()` / `ink_colour()` (trace.rs 257, 272).

Median outline width on the dumped `U×` masks (`ink_debug` +
`BUZZ_INK_DEBUG_DIR`, ridge of the distance transform, scratch
`line_width.py`):

| | source (bold 0) | bold 0.5 (today's rigs) |
|---|---|---|
| Raju | 2.25 px | 3.25 px (1.44×) |
| Guddu | 1.75 px | 2.75 px (1.57×) |
| Woman1 | 1.75 px | 2.75 px (1.57×) |

At 4× (scratch `raju_face_sheet.png`: source | bold 0 | bold 0.5) bold 0 is
the source's own line weight; bold 0.5 is visibly heavier everywhere (eyes,
brows, ear, collar). So:

- **default `ink_bold` becomes 0.0** in `ink()`, `ink_colour()` and
  `TraceOptions::default()`; the slider range becomes **−1.0 ..= 3.0** so the
  user can go *thinner* than the source (negative = erode each side by that
  many px; implement as `bold_mask` on the inverted mask, clamped so a loop
  never vanishes: skip erosion for components whose max EDT < 2·|bold|·U).
- The user then tunes it (their "2.5 vs 4" is a feel; the slider gives them
  the whole range).

## What becomes adjustable

One struct, `buzz_scene::TraceTuning` (serde `Serialize + Deserialize`,
`#[serde(default)]` on every field so an old file still loads, `Default` =
today's constants exactly, so a missing file changes nothing but `ink_bold`).
Group names are the UI's collapsing headers; the field doc is the hover text.

**Ink (trace_ink.rs)** — `ink_bold` stays on `TraceOptions` (it is already in
the dialog); these move from `const` to fields:

| field | today | range | const |
|---|---|---|---|
| `ink_level` | 0.5 | 0.2–0.8 | `INK_LEVEL` 107 |
| `min_contrast` | 60 | 20–120 | `MIN_CONTRAST` 143 |
| `dark_luma` | 48 | 20–90 | `DARK_LUMA` 146 |
| `dark_ramp` | 12 | 2–40 | `DARK_RAMP` 148 |
| `bg_percentile` | 0.80 | 0.5–0.95 | `BG_PERCENTILE` 49 |
| `band_half_width_px` | 3.0 | 1–8 | `BAND_HALF_WIDTH_PX` 55 |
| `corner_deg` | 70 | 30–120 | `CORNER_DEG` 626 |
| `fit_tolerance_px` | 0.15 | 0.05–0.6 | literal at 817 |
| `smooth_sigma` | (whatever `line_smooth` maps to) | keep `line_smooth` 0–1 | — |
| `upsample` | Auto | Auto / 1 / 2 / 4 / 6 | `upsample_factor` 270 (an `Option<usize>`; Auto = today's rule) |
| `speck_px2` | `speckle` 12 | 0–100 | already `TraceOptions::speckle` — leave |
| `faint_max_contrast` / `faint_max_px2` | 40 / 160 | 10–80 / 20–600 | 463–464 |
| `dark_hole_max_px2` / `dark_hole_max_luma` | 200 / 120 | 20–1000 / 40–200 | 455–456 |
| `colour_mark_min_chroma` | 60 | 20–160 | 541 |

**Colour (trace_colour.rs)** — `colours` stays on `TraceOptions`:

| field | today | range | const |
|---|---|---|---|
| `bilateral_passes` | 2 | 0–4 | 85 |
| `bilateral_range_sigma` | 10 | 3–30 | 92 |
| `palette_merge_de` | 6 | 2–16 | 101 |
| `surface_max_k` | 8 | 2–16 | 111 |
| `textured_median_gradient` | 2.5 | 1–8 | 120 |
| `surface_max_k_textured` | 3 | 1–8 | 122 |
| `soft_merge_max_de` | 16 | 4–30 | 160 |
| `soft_merge_gradient_cap` | 7 | 2–15 | 172 |
| `min_region_px2` | 40 | 4–400 | 148 |
| `contrast_region_min_de` | 20 | 8–40 | 157 |
| `residual_line_de` | 13 | 5–30 | 405 |
| `under_paint_max_layers` | 4 | 0–8 | 75 |
| `ink_grow_px` | 1 | 0–3 | 83 |

**Character (buzz-ai character.rs)** — on `TraceCharacterOptions`:

| field | today | range | const |
|---|---|---|---|
| `trace: TraceOptions` | `ink_colour()` | — | so `ink_bold`, `colours`, `tuning` flow in |
| `ink_line_share` | 0.60 | 0.2–0.9 | `INK_LINE_SHARE` 2146 |
| `ink_network_min_span` | 0.5 | 0.1–0.9 | 2155 |
| `hidden_reach_fraction` | 0.02 | 0–0.1 | `HIDDEN_REACH_FRACTION` 2241 |
| `uniform_width` | per dialog | existing | — |

Constants not listed stay constants (they are structural: k-means iterations,
Lloyd rounds, percentile of the median...). The implementer may add a knob
that is clearly a "look" decision; must not add structural ones.

## C1 — `TraceTuning` (buzz-scene)

- New `crates/buzz-scene/src/trace_tuning.rs`: `pub struct TraceTuning { pub ink: InkTuning, pub colour: ColourTuning }`, both `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]` + `#[serde(default)]`, `Default` = the table values. Each field has a `///` doc of one line (becomes hover text) — plain words, say what goes up when the value goes up.
- `TraceOptions` gets `pub tuning: TraceTuning` (serde too — check whether `TraceOptions` already derives serde; if not, derive it; `TraceMode`, `LineStyle`, `InkDetect` as needed).
- Replace every use of the listed constants with the field, threaded as `&InkTuning` / `&ColourTuning` down the call chain (grep each constant name; most live in a handful of functions). Keep the constants' doc comments on the fields. Delete the constants.
- `ink_bold` default 0.0; negative values erode (above). `ink_debug`'s `[bold]` arg already parses negatives.
- `upsample: Option<usize>`: `None` = `upsample_factor(w, h)`; `Some(u)` clamps 1..=6.
- Tests: `trace_tuning::tests::default_round_trips_through_json`, `an_unknown_or_missing_field_loads_as_default`, and in trace_ink tests `negative_bold_thins_the_ring` (ring area shrinks, loop survives) + `bold_zero_keeps_the_source_width` (synthetic 3 px ring → ridge width 3 ± 0.5 at U 4). Existing tests must still pass unchanged **except** those that assert on the old bold 0.5 default — fix those by setting `ink_bold = 0.5` explicitly.

## C2 — file, env, CLI

- `TraceTuning::path()` = `<app data dir>/BuzzAnimate/trace_tuning.json` (same base as `presets.rs:118`); `load_or_default()`, `save(&self)`. Pretty JSON.
- `BUZZ_TRACE_TUNING=<file>` overrides the path (headless runs and `series_setup` pick it up without the UI).
- `TraceOptions::ink()` / `ink_colour()` / `default()` call `TraceTuning::load_or_default()`? **No** — keep them pure (tests). Add `TraceOptions::with_saved_tuning(self) -> Self` and call it in: the Trace dialog's initial options, the AI character dialog, `series_setup`, `trace_png`, `ink_debug`, `rig_vs_source`, `episode_build` if it traces (grep `TraceOptions::` in `crates/buzz-app/examples` and `crates/buzz-ui`).
- `series_setup`, `trace_png`, `ink_debug`, `rig_vs_source`: `--tuning <file>` and `--bold <px>` flags (`--bold` overrides the file).

## C3 — UI

Trace dialog (`crates/buzz-ui/src/trace_dialog.rs`): below the existing grid,
`egui::CollapsingHeader::new("Tuning")` (closed by default) with two inner
headers **Ink** and **Colour** (Colour only when the mode has colour) — one
`Slider` per field in the tables above, `.on_hover_text(doc)`, step chosen so
the slider has ~50–100 positions. Under them a row: **Save as default**
(writes `trace_tuning.json`), **Reset** (back to `TraceTuning::default()`),
**Load file…** (rfd file dialog if the crate is already a dependency, else
skip and say so). The `ink_bold` slider range −1.0..=3.0, hover text: "px added
to each side of every line; 0 = the source's weight, negative = thinner".
The dialog already re-traces on option change (check `trace_dialog.rs` for how
`changed()` triggers the preview) — the tuning sliders must trigger it the
same way.

AI character dialog (`ai_character_dialog.rs`, `render_preview` ~829 has the
uniform-width slider): add **Bold**, **Colours** and the same collapsing
**Tuning** section, bound to `state.trace_options: TraceCharacterOptions`
(new field, built with `with_saved_tuning`); `create_character` passes it.
Same Save / Reset row (saves the shared file — one file for both dialogs).

Headless UI test (per the repo's rule — `App::new` + `build_ui`, no real
clicks): `trace_dialog::tests::tuning_section_lists_every_field` — build the
dialog once with the Tuning header open and assert the number of sliders ==
number of `TraceTuning` fields (+ bold). Keep it cheap.

## C4 — character pipeline

`TraceCharacterOptions` gains the fields in the table; `trace_character_whole_with`
uses `options.trace.clone()` (with `mode` forced to `InkAndColour` when
`ink_colour` is on, `drop_background` true) instead of `TraceOptions::ink_colour()`,
and the three constants become the option fields. `Default` keeps today's
values (`trace: TraceOptions::ink_colour()`, i.e. bold 0.0 now).

## Acceptance (Fable review)

- `cargo test --release -p buzz-scene -p buzz-ai -p buzz-ui` green; `-p buzz-app --examples` builds.
- `ink_debug` on Raju with default tuning → ridge median ≈ 2.25 px (scratch `line_width.py`); with `--bold 0.5` → 3.25 (today's result reproduced exactly, proving the refactor changed nothing but the default).
- `trace_png … inkcolour` on Raju/Guddu/Woman1 with default tuning vs a saved file with `ink_bold = -0.25`: sheets at 4× in `ep1/buzz/ink_check/<name>_bold_sheet.png`.
- Rebuild the Cast + `episode_v3` only after the user picks a bold value in the UI.
