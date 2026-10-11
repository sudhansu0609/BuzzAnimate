# Plan: hard light, split faces, lit cheekbones, and lights between layers

Planned by Opus (architect), 2026-09-25. Ground rules:
- Do not run `cargo fmt`.
- Never drive the real mouse; verify with headless renders and pixel
  measurements (`crates/buzz-render` and `crates/buzz-app` test patterns).
- Every stage ends with `cargo test -p buzz-light -p buzz-render -p buzz-app`
  green and the Status table below updated.

The ask, in the animator's words: lights all behave as soft light. A light
should be able to be **hard**: one side of the face bright, the other side
dark, with the cheekbone still catching light. Bringing the light round to the
front should brighten the whole face. A light should be placeable **between
two layers**, so a character with their back to the camera goes dark against
the lit thing behind them, a silhouette with the light in front of it.

---
## Findings: why every light reads as soft

All line numbers are as of 2026-09-25.

1. **The shaded side is a crescent whose width comes from the softness
   slider, never from where the light is.** `buzz-light/src/geometry.rs:81`
   `shade_width(softness)` is `softness.max(0.3).min(0.9)`, and
   `shade_crescent` (line 111) shifts the silhouette by that fraction of its
   *smaller* side and subtracts. So:
   - a hard light (softness 0) is also the *thinnest* band: 30% of the smaller
     side, tapering to nothing at the top and bottom of the form. A hard side
     light can never darken half a face.
   - the same slider also sets the feather (`shade_feather`, line 101), so
     "hard" and "narrow" are one knob.
2. **The band ignores how far in front the light stands.**
   `crescent_direction` (geometry.rs:563) returns only the normalised planar
   direction. A sun at 85° and a sun at 10° give a band of the same width;
   at exactly 90°, or a lamp exactly in front, the band vanishes in one step
   (`planar.hypot() > 1e-6`). Bringing a lamp round to the front therefore does
   not brighten the face progressively: the band stays full width until it
   pops off. The direct light *does* rise (`Light::direct_at`, lib.rs:894,
   uses `towards.z`), but the modelling does not follow it.
3. **A lamp cannot go behind anything.** The Forward slider is
   `4.0..=1200.0` (`buzz-ui/src/light_panel.rs:387`) and the on-stage stalk
   drag clamps to `MIN_LAMP_HEIGHT` (`buzz-app/src/lights.rs:431`). The maths
   already allows it: `Light::towards` (lib.rs:832) uses `height + depth`, and
   `direct_at` clamps `facing` to zero, so a lamp behind a layer already lights
   that layer with ambient only. Only the UI forbids it.
4. **A sun cannot backlight.** `direct_at` uses `towards.z = sin(elevation)`
   as N·L, which is "up", not "towards the camera". Azimuth only moves the
   floor shadow (`shadow_transform`, geometry.rs:240). There is no way to say
   "the sun is behind the character".
5. **A lamp's pool is always drawn over everything** (`draw_light_pools`,
   `buzz-render/src/document.rs:1773` and `1843`, at the focal plane). A lamp
   placed behind a character would still glow *over* the character, so there
   can be no silhouette against the light.
6. **When the light is behind, the crescents are wrong, not absent.**
   `crescent_direction` still returns a planar direction, so a figure behind
   a lamp gets a shade band on its far side and a glint on its near side as
   if front-lit, on top of an ambient-only fill. The correct picture is the
   whole figure in ambient with a thin lit **rim band** on the edge nearest the
   light.
7. **The rim glow is weakest exactly when it should be strongest.**
   `rim_glow` (geometry.rs:979) multiplies by `towards.z.max(0).max(0.35)`:
   a light behind the figure gets the 0.35 floor; a light in front gets full.
   Cinematically it is the other way round.
8. **Only one light models.** `LightRig::key` (lib.rs:1537) picks the
   strongest caster; only it draws crescents, casts shadows and rims. A key in
   front plus a kicker behind, the ordinary three-point setup, cannot be drawn.
9. **Nothing knows a face has a cheekbone.** All shading is derived from the
   silhouette, so it can only ever darken from the outline inwards. A
   cheekbone, nose, brow or chin catching light on the dark side (the
   Rembrandt triangle) needs form information the artwork does not carry.
   The face rig (`buzz-ai/src/face.rs:28` `FaceRigSymbols`) knows the two eye
   centres and the mouth centre at build time, which is enough to *guess*
   where the cheekbones are; nothing persists it.
10. **Cache keys.** Crescents are cached by `Aim { direction, softness }`
    (`buzz-render/src/lighting.rs:125`). Anything new that turns a crescent
    (the light's depth angle) must join that key or the cache serves stale
    bands.

## Decisions

- **The terminator comes from geometry; softness becomes feather only.**
  Let `lz` be the light's in-front component at the figure, in `-1..=1`
  (`towards.z` for a lamp; `sin(elevation)` for a sun, negated when the sun
  is flagged as behind). On a sphere lit from direction `L`, the dark part of
  the visible disc has thickness `R·(1 − lz)` along the light axis, so the
  shaded fraction of the diameter is `(1 − lz)/2`: nothing at `lz = 1`, half
  at `lz = 0`, everything at `lz = −1`. The shifted-copy crescent already
  produces a band whose maximum thickness equals the shift, so the shift
  becomes `extent · (1 − lz)/2` and the softness slider only feathers.
  A hard side light then darkens half a face with a razor terminator, and a
  lamp brought round to the front thins the band continuously to nothing.
- **Behind is the same formula.** For `lz < 0` the figure is filled at ambient
  (already true) and gets a **lit band** on the light's side of thickness
  `(1 + lz)/2` of the smaller side, screened in the light's colour at glint
  strength. At `lz → 0⁻` that is half lit / half dark, continuous with the
  front case.
- **A light has a depth, and layers compare against it.** Lamps get a signed
  Forward range; suns get a `behind` flag (a sun is at infinity, so it is
  behind everything or in front of everything). A lamp's pool is drawn at its
  place in the paint order, so layers in front of it occlude the glow.
- **Two lights model, not one:** the strongest light *in front* of the figure
  draws shade and glint; the strongest light *behind* it draws the rim band
  and the rim glow. One extra crescent per figure, only when a back light
  exists.
- **Cheekbones are Form guides ("bumps") on a symbol.** An ellipse the
  artist places (or the app guesses from a face rig) is treated as a small
  sphere: its own lit crescent, screened in the light's colour, clipped to the
  figure. On the dark side of a hard-lit face that is the patch of light on
  the cheek. Stored on the `Symbol` so every pose and instance carries it.
- **Old files keep their look.** A new `LightRig::model` enum
  (`Crescent` = today, `Form` = this plan) defaults to `Crescent` when missing
  from a file and to `Form` for new documents. Everything below is gated on
  `Form` unless stated.

**Order of work:** Stage A, then B, then C. A and B are the ask; C is the
cheekbone and is the only one that adds authoring UI.

---
## Stage A. Hard light: the terminator from geometry

### Steps
1. `buzz-light/src/lib.rs`: add to `LightRig`
   `#[serde(default)] pub model: ShadeModel` with
   `enum ShadeModel { #[default] Crescent, Form }` (serde default Crescent;
   `LightRig::default()` sets Form). Hash it in `fingerprint` and `aim`.
   Expose `label()` and `ALL` like `EdgeMode`.
2. `buzz-light/src/lib.rs`: add `Light::facing_at(point, depth) -> Option<f64>`
   returning `lz`: `towards.z` for a lamp; `sin(elevation)` for a sun. (Stage
   B negates it for a sun flagged behind.) Add
   `Light::aim_at(point, depth) -> Option<Aim3 { planar: Vec2, lz: f64 }>`
   built from `screen_towards` and `facing_at`. Keep `crescent_direction`
   for the Crescent model; add `crescent_aim(light, at, depth, modelling)
   -> Option<Aim3>` for Form, which returns `None` only when modelling is off.
3. `buzz-light/src/geometry.rs`:
   - `pub fn form_shade_width(lz: f64) -> f64` = `((1.0 − lz) * 0.5).clamp(0.0, 0.9)`;
   - `pub fn form_lit_width(lz: f64) -> f64` = `((1.0 + lz) * 0.5).clamp(0.0, 0.9)`;
   - `shade_crescent_form(path, planar, lz)` and `highlight_crescent_form`
     use those widths in `crescent_offset`; the glint keeps `HIGHLIGHT_SHARE`
     of the *shade* width so a hard light still has a narrow glint. When
     `form_shade_width < 0.02` return `None` (the band has vanished: a light
     straight in front).
   - `crescents_form(path, aim: Aim3, softness) -> ShadeGeometry`: for
     `lz >= 0` shade + highlight as above; for `lz < 0` shade `None`, and
     `highlight` = the lit band of `form_lit_width(lz)` on the light's side.
   - `shade_reach`/`highlight_reach` gain `_form` variants taking `lz`, used by
     the renderer's feather ramps.
4. `buzz-render/src/lighting.rs`: `Aim` gains `facing: i8` (lz quantised to
   1/32). `Aim::of(towards, softness)` becomes `Aim::of(towards, lz, softness)`;
   every `ready`/`crescents`/`figure`/`Miss` call threads `lz` through
   (`Some(lz)` for Form, `None`→0 for Crescent). Workers build with
   `crescents_form` when a facing is present.
5. `buzz-render/src/document.rs`: at each `crescent_direction` call
   (figure pass 3293, shape pass 4946 and 5256) branch on `ctx.lights.model`
   and pass the aim through. Feather ramps (`banded_ramp` callers at 4993,
   5419) take the Form reach. The `lz < 0` case draws no shade band and
   screens the highlight band at `Illumination::highlight_strength(glint)`
   in the light's colour.
6. `buzz-ui/src/light_panel.rs`: rename the Softness tooltip to say it is
   the feather; add a **Model** selector next to Edges (Crescent / Form) and
   two preset buttons on a light: **Hard** (softness 0.05) and **Soft**
   (softness 0.8). The app command layer (`buzz-app/src/lights.rs`) already
   round-trips edits via `LightRig` clone; extend whatever `LightPanelOut`
   carries `edges` to carry `model`.
7. `buzz-doc/src/serial.rs`: round-trip `model`; a file without it loads
   `Crescent`.

### Tests
- `buzz-light`: `a_side_light_darkens_half_the_form` (disc, lamp at
  `lz = 0`: shade crescent area ≈ 50% ± 5% of the disc);
  `a_front_light_leaves_no_band` (lz ≥ 0.96 → `None`);
  `a_light_behind_leaves_a_lit_band_not_a_shade`;
  `the_band_thins_continuously_as_the_light_comes_forward` (widths at
  lz = 0, 0.25, 0.5, 0.75 strictly decreasing);
  `softness_no_longer_changes_width_in_form_model`.
- `buzz-render`: headless render of a grey disc, Form model, hard lamp at the
  left, `lz = 0`: mean luma of the left third > 1.6 × mean luma of the right
  third, and the column where luma drops crosses within ±6% of the centre;
  the same with softness 0.05 vs 0.8: the transition width (columns between
  10% and 90% of the drop) is < 4 px hard and > 20 px soft.
- `buzz-doc`: `a_file_without_a_model_loads_as_crescent`.

### Risks
- The shape pass (per-shape crescents, `EdgeMode::Shapes`) uses each shape's
  own bounds, so a hundred-shape character in Shapes mode will get a half-dark
  band per shape. That is the existing Shapes behaviour, only wider; Figure
  mode is the default and is the one this targets.
- A figure whose bounding box is much wider than its body (arm out) gets a
  narrower band than a sphere would; accepted for v1.

---
## Stage B. Lights behind layers: silhouettes and rims

### Steps
1. **Lamp depth range.** `buzz-ui/src/light_panel.rs:387`: Forward becomes
   `-(scene.receiving_depth() + 400.0)..=1200.0`, labelled **Depth**, with
   the tooltip stating that negative is behind the stage and which layers it
   sits behind. Under the slider show one line: "Behind: <layers with
   `depth < −height`>" (names, up to three then "…").
   `buzz-app/src/lights.rs:431`: `MIN_LAMP_HEIGHT` becomes the same negative
   bound (take the scene in; the gesture already has it).
2. **Sun behind.** `LightKind::Sun { azimuth, elevation, #[serde(default)]
   behind: bool }`. `facing_at` returns `−sin(elevation)` when behind.
   `direct_at` for a sun uses `facing_at` (so a behind sun delivers no direct
   light); `shadow_transform` is unchanged (azimuth still says where the floor
   shadow goes). Panel: a **Backlight** checkbox on the sun. Hash `behind`
   in `hash_into` and `aim`.
3. **Pool at its depth.** `buzz-render/src/document.rs:1689` builds
   `ordered`; the layer loop draws each layer. Add
   `fn pools_before(layer_depth: f64, lights) -> Vec<&Light>`: a lamp with
   `height + layer.depth < 0` is behind that layer. Draw each lamp's pool
   immediately before the first layer in paint order that it is behind; lamps
   behind no layer keep the current end-of-frame draw. Gloom bands stay at
   the end. Keep the pool at the focal plane projection (no parallax) in v1.
4. **Second modelling light.** `LightRig::key()` is renamed in meaning:
   add `LightRig::key_at(point, depth) -> Option<&Light>` (strongest caster
   with `facing_at >= 0`) and `LightRig::back_at(point, depth)` (strongest
   with `facing_at < 0`). The figure pass (`FigurePass::open_over`, 3264)
   asks for both: `key` builds shade + glint as Stage A; `back` builds the
   lit rim band via `crescents_form` with its negative `lz`, cached under its
   own `Aim`. `close` draws the key's bands, then the back's band screened in
   the back light's colour. Shadows keep using the front key only
   (`shadow_throw` callers at 2262). Where the Crescent model is active,
   nothing changes.
5. **Rim glow strongest from behind.** `rim_glow` (geometry.rs:979): replace
   `facing.max(0.35)` with `((1.0 − lz) * 0.5).clamp(0.35, 1.0) as f32`
   where `lz = towards.z`. The layer rim (document.rs:2439) asks
   `back_at` first, then `key_at`.
6. **Shadow of a caster whose light is behind it.** `shadow_transform`
   already returns `None` for a lamp at or below the caster (`gap <= 1.0`).
   Confirm `ShadowThrow::Ground` with `lamp: Some((position, height))` also
   bails when `height <= 0`; add the guard if not. A forward-falling shadow is
   a follow-up.
7. **Timeline Depth view** (`buzz-ui/src/timeline_panel.rs:628`): when the
   depth view is on, draw a small lamp marker in the layer column at each
   lamp's depth row, so "between two layers" is visible where layers are
   ordered. Read-only in v1.

### Tests
- `buzz-light`: `a_lamp_behind_a_layer_lights_it_with_ambient_only`
  (`direct_at` = 0 for `height + depth < 0`);
  `a_backlit_sun_delivers_no_direct_light_and_a_full_rim`;
  `key_and_back_are_chosen_by_facing`.
- `buzz-render` headless: stage with a background layer at depth 600 and a
  character disc at depth 0; lamp at height −300 (between them), Form
  model. Assert: (a) a pixel on the background beside the disc is brighter
  with the lamp than without; (b) a pixel at the disc centre is *not* tinted
  by the pool colour and equals the ambient-only fill; (c) the disc's edge
  column nearest the lamp is brighter than its centre (rim band); (d) moving
  the lamp to height +300 restores the Stage A front-lit picture.
- `buzz-render`: `a_pool_is_drawn_under_the_layers_in_front_of_it`: the pool
  colour is present outside the disc and absent inside it.
- `buzz-ui`: the Depth slider accepts a negative value and the panel emits
  the edit; the sun panel round-trips `behind`.
- `buzz-doc`: sun `behind` survives the round trip; a file without it loads
  `false`.

### Risks
- Drawing a pool mid-stack adds a fill inside the layer loop; it is one
  circle, uncached, same as today.
- `key()` is called in several places (`illuminate`, `field`, shadows). Only
  the modelling and rim call sites change; keep `key()` as "strongest
  caster" for shadows so shadow behaviour is untouched.

---
## Stage C. Cheekbones: Form guides

### Steps
1. `buzz-scene/src/symbol.rs` `Symbol`: add
   `#[serde(default)] pub form: Vec<FormGuide>` with
   `struct FormGuide { pub name: String, pub centre: Point, pub radii: Vec2,
   pub angle: f64, pub height: f32 /* 0..=1, how far it stands proud */ }`.
   Round-trip in `buzz-doc/src/serial.rs`. Bump the symbol content hash used
   by `figure_content` (document.rs) so edits rebuild.
2. `buzz-light/src/geometry.rs`: `pub fn bump_bands(guide: &FormGuide,
   aim: Aim3, softness) -> ShadeGeometry`: the guide's ellipse as a path; its
   lit crescent = ellipse minus ellipse shifted *away* from the light by
   `min(radii) · form_shade_width(lz) · height`; its shade crescent the
   mirror. For `lz < 0` only the lit crescent, of `form_lit_width`.
3. `buzz-render/src/document.rs` figure pass `close`: after the figure's
   bands, for each guide of each symbol instance among `members` (transform
   by the instance matrix), clip the bump's bands to the figure silhouette
   and draw: lit crescent screened in the light's colour at
   `highlight_strength(glint)`, shade crescent multiplied by `shade_filter`
   at half strength. Cached per instance under the same `Aim` machinery
   (`LightCache::FIGURE + 1` slot index).
4. **Guessing a face.** `buzz-ai/src/face.rs` `build_face_rig` knows the eye
   centres and mouth centre: after building, write four guides onto the head
   symbol: `Cheek L/R` at `eye_x ± 0.55·eye_gap`, `eye_y + 0.45·(mouth_y −
   eye_y)`, radii `(0.30·eye_gap, 0.22·eye_gap)`; `Nose` between the eyes at
   `eye_y + 0.30·(mouth_y − eye_y)`, radii `(0.16, 0.28)·eye_gap`;
   `Brow` above the eyes, `Chin` below the mouth. Also expose
   `App` command `GuessFormGuides` for a selected symbol with children named
   like eyes/mouth (case-insensitive contains "eye"/"mouth"), same maths.
5. **Authoring.** In the Properties panel for a selected symbol instance, a
   **Form** section: list of guides with name and a delete button; **Add
   bump** places a guide at the selection centre at a quarter of the
   bounds; on-stage, guides show as dashed ellipses with a centre handle
   (move) and one radius handle (resize) only while the Form section is
   open. Undoable through the usual edit command path.
6. `LightRig::model == Crescent` ignores guides entirely.

### Tests
- `buzz-light`: `a_bump_on_the_dark_side_has_a_lit_crescent_facing_the_light`.
- `buzz-render` headless: a disc "face" with a cheek guide on the dark side,
  hard lamp from the left at `lz = 0.2`: the mean luma inside the guide's lit
  crescent is > 1.3 × the mean luma of the dark side around it.
- `buzz-ai`: `build_face_rig` yields four guides with the cheeks below and
  outside the eyes.
- `buzz-app` headless: `GuessFormGuides` on a symbol with `Eye L`, `Eye R`,
  `Mouth` children adds guides; undo removes them.

### Risks
- Guides are ellipses in symbol space; a rig bone that deforms the head
  non-affinely will not carry them exactly. Accept: heads are usually one
  part.
- Clipping every bump to the figure silhouette is a boolean per bump per
  rebuild; four per face is negligible next to the figure's own crescents.

---
## Verification (Opus, after each stage)
- Run the stage's tests and the full `cargo test -p buzz-light -p buzz-render
  -p buzz-app -p buzz-doc -p buzz-ui`.
- Render the measurement fixtures to PNG (`buzz-app/examples/wave_sheet.rs`
  shows the headless export path) and inspect: side-lit disc, lamp between
  layers, cheek guide. Record the measured ratios in the Status table.
- No visible change to an existing document that has no `model` field.

## Status

| Stage | State | Tests | Measured | Notes |
| --- | --- | --- | --- | --- |
| A. Terminator from geometry | **Verified through the app.** The app-level wire (`app.rs`'s `light_panel` handler's `if let Some(model) = response.set_model { ... }` arm) that the previous review found pending is now present and tested; one further bug found and fixed while verifying end to end (see Notes). | `buzz-light`: `a_side_light_darkens_half_the_form`, `a_front_light_leaves_no_band`, `a_light_behind_leaves_a_lit_band_not_a_shade`, `the_band_thins_continuously_as_the_light_comes_forward`, `softness_no_longer_changes_width_in_form_model` (all in `geometry::form_model_tests`) — pass. `buzz-render` (`tests/hard_light.rs`, GPU, headless): `a_side_light_darkens_the_far_side_of_a_disc`, `the_crescent_model_still_gives_a_narrow_hard_band`, `a_hard_terminator_is_narrower_than_a_soft_one` — pass. `buzz-doc`: `a_file_without_a_model_loads_as_crescent` — pass. `buzz-render/lighting.rs` (16 tests, incl. the cache tests below) — pass. **New, through the app**: `buzz-ui/src/light_panel.rs`'s `clicking_model_form_and_hard_sets_both` (the Model row and the Hard/Soft presets exist and work — pass); `buzz-app/src/app.rs`'s `a_brand_new_document_starts_on_the_form_model`, `an_imported_document_also_starts_on_the_form_model` (pass); `buzz-app/tests/hard_light_app.rs`'s `a_hard_side_light_darkens_the_far_side_through_the_app`, `a_soft_side_light_has_a_wide_transition_through_the_app`, `the_crescent_model_still_gives_a_narrow_band_through_the_app`, `switching_crescent_to_form_on_a_warm_cache_updates_the_stage` — all pass (added as part of this fix; the last one is a regression test for the bug below). | Through the app (`Editor::add_light` + the same `doc.edit` calls the panel's handler runs, rendered via both `buzz_export::Exporter` and `stage::build_scene`), a 300px disc, sun at azimuth 180°/elevation 10° (`lz \u{2248} 0.174`), Form, softness 0.05 (Hard preset): lit/shaded-third luma ratio **11.06** (asked > 1.6); terminator **9.3%** of the diameter from centre (asked < 8% in the brief; see Notes for why 10% is the honest bound at this `lz`, same as the render crate's own 9%); hard transition **3.4px**, soft (0.8) transition **62.8px** (asked < 4px / > 20px) — export and stage agree pixel-for-pixel on all three numbers. Crescent model, same disc, same hard softness: a point 0.28 radii past centre (inside Form's shaded side, outside Crescent's) reads within 1.3x of the lit side — confirms Crescent is unchanged. PNGs: `target/hard_light/app_disc_hard.png`, `app_disc_hard_stage.png`, `app_disc_soft.png`, `app_disc_crescent.png`. | Steps 1–7 all done, including Step 6's app wire (`buzz-app/src/app.rs`: `if let Some(model) = response.set_model { editor.doc.edit("Light Model", \|scene\| scene.lights_mut().model = model); }`) — present and exercised by `clicking_model_form_and_hard_sets_both` and the `hard_light_app.rs` suite. **Bug found and fixed**: `buzz-render/src/lighting.rs`'s crescent cache keys a built crescent on `Aim { direction, softness, facing }`, and `facing` was a bare `i8` that both models could round to `0` — Crescent always did (it never reads `lz`), and Form did too whenever the light sat within about a degree of exactly to the side (`lz \u{2248} 0`, e.g. a sun levelled to `elevation = 0` — the single most natural way to try "a hard side light"). A document opened under the old Crescent default, looked at once (warming the cache), then switched to Form in the panel with nothing else touched, kept showing the old narrow Crescent band forever: the Form request hit the stale Crescent entry already sitting under that exact key. Fixed by making `Aim::facing` an `Option<i8>` (`None` for Crescent, `Some(_)` for Form) so the two models can never share a cache key; regression test `switching_crescent_to_form_on_a_warm_cache_updates_the_stage` reproduces it (left/right ratio stuck at 4.88 before the fix, 4.88 \u{2192} 7.23 after) and is now green. This is very likely the report: the panel, the defaults, and the renderer were all already correct in isolation, but a document that had been *looked at* under Crescent before the Model switch would appear unchanged. Defaults path confirmed: `Scene::default()` (a brand-new document, and — since `import::read` never touches `lights` — an imported `.fla` too) starts `ShadeModel::Form`; a `.buzz` file with no `model` field loads `Crescent` (`buzz-doc`). Live-stage cache invalidation (brief's step 4) is otherwise sound: `Aim` already threaded `lz`/`facing` through `ready`/`crescents`/`figure`/`Miss`/`Built`, and `LightRig::aim()` already hashes `model`; only the one collision above was wrong. `LightDetail` was not exercised by a document this light (one disc) — Full throughout — so the "never drops the Form band on a light document" clause is untested at the budget-pressure end; not touched in this pass. `cargo test -p buzz-light -p buzz-render -p buzz-doc -p buzz-ui -p buzz-app` was not re-run in full (large pre-existing unrelated churn on this branch per earlier reviews); everything listed under Tests above was run individually and is green. |
| B. Lights behind layers | Not started | | | |
| C. Form guides (cheekbones) | Not started | | | |
