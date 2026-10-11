# Plan: fluid-brush viscosity, per-brush tip shapes, line-art Trace Bitmap, Break Apart

Planned by Opus (architect), 2026-09-23. Order: 1, then 2 (the same files), then 3
(uses the `tip` field from 2); 4 is independent. No `cargo fmt`. Run
`cargo test -p <crate>` after each step.

**Decisions taken for the user's open questions** (the user may override them):
- Keep the viscosity default at 0.65, so pooling becomes visible. Existing width
  tests pin `viscosity = 1.0` in their fixtures.
- The tips for v1 are Round, Square, Chisel and Ellipse. A custom drawn nib is
  deferred.
- Imported bitmaps keep arriving already broken apart (`Scene::place_image`).
  Break Apart turns tiling on, which is Animate's behaviour.
- Line art defaults to **variable-width brush strokes** (`LineStyle::Brush`),
  because the user asked for "the exact effect of the brush tool".
- A blob fused to a line is traced as one filled outline in v1.

---
## 1. Fluid brush viscosity has no visible effect

### Findings
- The chain is wired end to end:
  - the slider at `buzz-ui/src/panels.rs:1600-1609` sets `BrushSettings::viscosity`
    (`buzz-ui/src/brush.rs:239`, default 0.65 at :310);
  - `BrushSettings::profile()` clamps it (`brush.rs:354`);
  - `buzz-app/src/tools.rs:1544 brush_profile_for` leads to
    `tools.rs:1484 build_brush_path` (Fluid and Normal at :1494);
  - that calls `buzz_geom::fluid_outline` (`buzz-geom/src/brush.rs:608`);
  - which uses `BrushProfile::outline_tension` (:227, `1 - v*0.9`) and then
    `catmull_rom_tense` on the left and right offset points (:645-647).
- It is not commit-only. The preview (`tools.rs:876-884`) and the commit
  (`tools.rs:1073-1074`) both call `vector_brush_shape` with `BrushBudget::default()`.
- **Root cause:** viscosity only scales the Catmull-Rom tangents between
  neighbouring offset points. Those handles are already capped at chord/3
  (`brush.rs:579-587`). After decimation the points are 0.75 to about 6 units
  apart (`min_spacing` 0.75 at :258) and already pre-smoothed (default smoothing
  0.5), so the largest difference between v=0 and v=1 is about 0.25 × chord:
  sub-pixel. The unit test `a_viscous_brush_spreads_less_than_a_runny_one`
  (:1436) passes only because it uses 4-unit spacing, a wobble of ±26 and
  smoothing 0, which masks the problem.

### Design
Keep the tension, and add a visible effect at the scale of the stroke width
with the same meaning (runny paint spreads): **paint pools where the brush
lingers.**
- `spread = 1 - viscosity.clamp(0,1)`.
- `linger_i = 1 - clamp(speed_i / POOL_SPEED, 0, 1)`, with `POOL_SPEED = 400.0`
  document units per second.
- Box-average `linger` over arc length, within ±`width/2`.
- `width_i *= 1 + MAX_POOL * spread * linger_i`, with `MAX_POOL = 0.6`.

The pooling is deterministic from the samples, so the preview equals the
commit. It scales with width, so it is visible at every size. At v=1 it is
exactly zero, which keeps the promise that viscous paint keeps its drawn width.

### Steps
1. `buzz-geom/src/brush.rs`:
   - next to `BrushProfile` (:137), add `const POOL_SPEED: f64 = 400.0;`,
     `const MAX_POOL: f64 = 0.6;` and `fn spread(&self) -> f64`;
   - rewrite the doc comment at :154-167; the old claim that viscosity is
     "never how wide it is" must go.
2. `widths_along` (:685):
   - collect the existing per-sample `speed` (:703-710) into a Vec;
   - if `spread > 0`, compute `linger` and smooth it with a two-pointer
     box-car over the existing `distances` Vec (radius
     `profile.width.max(1.0) / 2`), which is O(n);
   - multiply the width **before** the taper block (:715), so tapers still
     reach a point.
3. The tap branch (:616-620): `radius *= 1 + MAX_POOL * spread`.
4. `buzz-ui/src/panels.rs:1603-1608`: change the hover text to "Thin paint
   pools where the brush slows and bulges between points; thick paint keeps
   the width you drew."

### Tests
In the `buzz-geom brush.rs` tests:
- `viscosity_pools_paint_where_the_brush_lingers`: 150 samples at 2-unit steps
  along x, with a time step that gives 60 u/s. Width 20, `Uniform`, smoothing
  0, taper 0. Assert `area(0.0) >= 1.35 * area(1.0)` and
  `area(1.0) < area(0.5) < area(0.0)`. Reuse the shoelace closure at :1458-1469.
- `a_fast_stroke_barely_pools`: the same stroke at 2000 u/s, with
  `|area0 / area1 - 1| < 0.03`.
- `full_viscosity_keeps_the_drawn_width`: at v=1 on a slow stroke, the
  bounding-box height is 20 ± 0.5.
- `viscosity_does_not_move_the_stroke` (:1488): if it fails, loosen the
  tolerance to `2.0 + MAX_POOL * profile.width / 2` and add a comment saying
  why (the cap radius grows; the ends of the centreline stay pinned).

In the `buzz-app/src/editor.rs` tests (next to
`a_pens_pressure_reaches_the_stroke`, :10215):
- `viscosity_changes_a_drawn_stroke`: Normal brush, size 30, smoothing 0,
  taper 0. Call `pointer_down_at`, then `pointer_move_at` 40 times over 300
  units spanning 4.0 s, then `pointer_up_at`. The height at v=0 must be more
  than 1.25 × the height at v=1.
- `the_preview_is_the_commit_at_zero_viscosity`: the in-flight
  `Preview::Artwork` path equals the committed shape's path.

### Risks
Width-asserting tests that use the default 0.65 may drift. Set
`viscosity = 1.0` in those fixtures; do not weaken the assertions.

---
## 2. Separate tip shapes for the Fluid and Normal brushes

### Findings
- No tip model exists. The round cap is hard-coded: `append_cap`
  (`brush.rs:764`) is used at :652-668, and a tap is always a circle
  (:616-620).
- Fluid and Normal share one `BrushSettings` (`buzz-ui/src/brush.rs:224-299`)
  and one construction path (`is_outlined` :111; `tools.rs:1494`). They differ
  only in `response` (`brush.rs:343-349`).
- `BrushSettings` is not persisted. Per-person preferences live in `Workspace`
  (`buzz-ui/src/workspace.rs:326`) as `#[serde(default)]` fields, loaded by
  `load_from` (:1996) and saved by `save_to` (:2013). `buzz-geom` already
  depends on serde.

### Design
In `buzz-geom/src/brush.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum TipShape { #[default] Round, Square, Chisel { angle: f64, thickness: f64 }, Ellipse { angle: f64, thickness: f64 } }
```
- `angle` is the nib's long axis in degrees from +x.
- `thickness` is 0.05..=1, the short axis as a fraction of the long one.
- Provide `label()` and `KINDS` for the combo box.
- Provide `across(self, width, tangent) -> f64`, the width swept perpendicular
  to travel. With nib axis `a = (cosθ, sinθ)`, `b = (-sinθ, cosθ)`, unit
  normal `n`, `W = width`, `T = width*thickness`:
  - Round and Square: `W`;
  - Chisel: `W|a·n| + T|b·n|`;
  - Ellipse: `sqrt((W a·n)² + (T b·n)²)`.

### Steps
1. `BrushProfile` (:137): add `pub tip: TipShape`, defaulting to `Round` (:170).
2. `fluid_outline` (:608):
   - at :631-636, derive the tangent from the normal (`Vec2::new(n.y, -n.x)`)
     and set `half = profile.tip.across(widths[i], t) / 2.0`;
   - caps at :652-668: Round and Ellipse use `append_cap`; Square uses a new
     `append_square_cap(path, centre, from, to)` (from, from+t·r, to+t·r, to);
     Chisel uses a plain `line_to`;
   - the tap (:616): Round is a circle; Square is a square of side `width`;
     Chisel and Ellipse are a rect or ellipse of `W × T` rotated to `angle`;
   - export `TipShape` from `buzz-geom/src/lib.rs`.
3. `buzz-ui/src/brush.rs` `BrushSettings`:
   - add `pub fluid_tip: TipShape` and `pub normal_tip: TipShape` (both Round);
   - `pub fn tip(&self)` matches on `kind`: Fluid gives `fluid_tip`, Normal
     gives `normal_tip`, anything else Round;
   - add `pub fn tip_mut(&mut self) -> Option<&mut TipShape>`;
   - `profile()` (:337) sets `tip: self.tip()`;
   - add `#[derive(... Serialize, Deserialize)] pub struct BrushTips { pub fluid: TipShape, pub normal: TipShape }`
     and re-export it;
   - fix any exhaustive `BrushProfile { .. }` literals the compiler flags.
4. `panels.rs` `brush_properties`, inside the `is_outlined()` block (:1600),
   after Viscosity:
   - a "Tip" ComboBox (`id_salt(('brush-tip', style.brush.kind.label()))`)
     that edits `*style.brush.tip_mut().unwrap()`;
   - choosing Chisel or Ellipse seeds `angle: 45.0, thickness: 0.25`;
   - for Chisel and Ellipse, add an "Angle" slider (-90..=90°) and a
     "Thickness" slider (0.05..=1.0).
5. `workspace.rs:326`: add `#[serde(default)] pub brush_tips: crate::brush::BrushTips`.
6. `buzz-app/src/editor.rs`: in `Editor::new`, after `Workspace::load()`
   (:358), copy the tips into `style.brush`. Add
   `pub fn remember_brush_tips(&mut self) -> bool`, which copies the style
   tips into `workspace.brush_tips` and says whether they changed.
7. `buzz-app/src/app.rs`: right after the `tool_options_panel(` call, add
   `if self.editor.remember_brush_tips() { self.editor.workspace.save(); }`.

WYSIWYG: the tip reaches the geometry only through `profile()` and
`build_brush_path`. The on-stage brush-size cursor ring stays round (out of
scope).

### Tests
- `buzz-geom`:
  - `a_chisel_is_thin_along_its_nib_and_wide_across_it`: angle 0, thickness
    0.2, width 20. A horizontal stroke's bounding-box height is 4 ± 1; a
    vertical stroke's bounding-box width is 20 ± 1.
  - `a_square_tip_has_square_ends`: the point (end + 0.45W, +0.45W) is inside
    the Square outline but not inside the Round one.
  - `round_tip_is_the_old_stroke`.
- `buzz-ui`: `each_outlined_brush_keeps_its_own_tip` (Fluid→Chisel,
  Normal→Square, switching back and forth keeps each).
- `workspace.rs`: `brush_tips_round_trip_and_old_layouts_load`.
- `editor.rs`:
  - `remember_brush_tips` returns true once, then false;
  - the same drag with Normal+Chisel and with Fluid+Round gives different
    heights.

---
## 3. Trace Bitmap as line art (centreline strokes)

### Findings
- `buzz-scene/src/trace.rs` only traces regions (:32-39). The pipeline is:
  - `quantise` (:343);
  - a 4-connected flood (:200-229);
  - `outline` (:288), using `bucket::trace_contours`, `bucket::simplify` and
    `buzz_geom::smooth` (`edit.rs:125`).

  Options are at :50-82; the presets `default` and `line_art()` at :84-108.
- `Editor::trace_selection` (`editor.rs:7902`) maps pixels to the stage with
  `place = object.transform * fill.transform * scale(1/w,1/h)` (:7937-7939).
  At :7964 it transforms `shape.path` only, so a `StrokeSpec.width` would
  **not** be scaled.
- Commands `TraceBitmap` and `TraceLineArt` exist (`command.rs:132-135, 452-453, 641, 850, 1036`),
  with the menu at `panels.rs:616` and dispatch at `editor.rs:3784-3785`.
  There is no options UI.
- `tests/trace_bitmap.rs` uses a 64 px disc of radius 20, and
  `TraceLineArt` must give exactly 1 shape (:130).

### Design
**TraceOptions** (`trace.rs:50`): keep it `Copy`, add serde with
`#[serde(default)]`, and add these fields:
- `mode: TraceMode { Outline, LineArt, Both }`, default Outline;
- `line_smooth` 0..1, default 0.5;
- `min_line_length` in px, default 8;
- `line_style: LineStyle { Stroke, Brush }`, default **Brush**;
- `fill_enclosed: bool`, default false;
- `max_line_radius` in px, default 8, beyond which ink counts as a mass;
- `ink_threshold: Option<u8>`, default None, which means Otsu.

Presets: `line_art()` becomes `mode: LineArt, line_style: Brush, fill_enclosed: true`.
`Default` stays Outline, so `TraceBitmap` is unchanged. `TraceReport` gains
`strokes` and `fills`. `trace()` dispatches on the mode: Outline is the
existing code, LineArt is the new module, and Both runs the regions, then the
lines.

**New module `buzz-scene/src/trace_lines.rs`**, in pixel space, deterministic,
with no RNG:
- A. **Ink mask.** Composite over white (`L = luma*a + 255*(1-a)`), blur 3×3
  with [1,2,1]⊗[1,2,1]/16, then threshold with `ink_threshold` or Otsu (256
  bins; 128 if the histogram is degenerate). Ink is `L < t`.
- B. **Clean.** 8-connected ink components smaller than `speckle` become
  paper; enclosed 4-connected paper holes of 4 px or less become ink.
- C. **Exact Euclidean distance transform** (Felzenszwalb-Huttenlocher, two
  1-D passes). `r[p]` is the distance to paper; outside the image counts as
  paper.
- D. **Masses.** An 8-connected ink component with `max r > max_line_radius`
  is traced with the existing `outline()` and coloured by the mean of its
  pixels, and is excluded from the skeleton. This keeps the disc tests green.
- E. **Skeleton.** Zhang-Suen until nothing changes (cap `max(w,h)/2+1`
  iterations), then one staircase-cleanup pass.
- F. **Graph.**
  - Junctions are pixels with degree ≥ 3; adjacent junction pixels cluster
    into one node at their centroid.
  - Walk from each node through degree-2 pixels to the next node, recording
    pixel centres (+0.5), with the node centroids at both ends.
  - Leftover degree-2 cycles become closed chains.
- G. **Prune.** Drop spurs (edges with a degree-1 end) shorter than
  `max(min_line_length, 2*r_junction)` (two passes), then join edges at
  nodes left with degree 2.
- H. **Continuation at junctions.** Take each edge's tangent over its first
  `k = clamp(round(2r)+3,4,12)` px, greedily pair the most opposed tangents
  (more than 150° apart), and concatenate the pairs. A T gives 2 strokes and
  an X gives 2.
- I. Drop open chains shorter than `min_line_length`.
- J. **Width and smoothing.**
  - `w_i = max(1, 2r_i - 1)`, filtered by median-5 then mean-5.
  - Convert to `StrokeSample::with_pressure(p, w_i/w_max, arc/1000)` and apply
    `buzz_geom::brush::smooth_samples(samples, line_smooth)`
    `1 + (line_smooth*3) as usize` times, with endpoints pinned (closed chains
    pad with 3 wrapped samples).
  - Simplify with a new f64 Douglas-Peucker `simplify_points`
    (`tol = 0.35 + 1.15*line_smooth` px), then fit with `buzz_geom::catmull_rom`
    (closed chains get `close_path`). Do not use Schneider or the kurbo fitter
    (overshoot; see `brush.rs:16-25`).
- K. **Output.** The colour is the mean original RGB along the chain.
  - **Stroke** style: `ShapeData { path: centreline, fill: None, stroke: Some(StrokeSpec::new(colour, median w)) }`.
  - **Brush** style: `buzz_geom::fluid_outline(&smoothed_samples, &BrushProfile { width: w_max, min_ratio: 0.0, response: Pressure, smoothing: 0.0, stabiliser: 0.0, taper: 0.0, taper_ends: Neither, viscosity: 1.0, tip: Round }, &BrushBudget::default())`,
    then `ShapeData::filled(path, colour)` with non-zero fill.
- L. **Enclosed fills** (when `fill_enclosed` is on and the mode isn't
  Outline).
  - Flood the outside from the border and from transparent pixels.
  - Each remaining paper component of at least `speckle` px is dilated into
    the ink by `ceil(median r)+1`, so the fill tucks under the line.
  - Trace it with `outline()` and colour it with the mean of the undilated
    region.
- M. **Order.** Outline regions (Both mode) largest first, then the enclosed
  fills, then masses, then lines, longest first.
- N. **Budget.** If `w*h > 6_000_000`, box-downsample by
  `k = ceil(sqrt(wh/6e6))` and scale back up. Cap at 20,000 chains and say so
  in `message`.

### Steps
1. `trace.rs`:
   - add the fields, enums, serde and presets;
   - branch in `trace()`;
   - make `outline` `pub(crate)`;
   - export `TraceMode` and `LineStyle` from `lib.rs`.
2. Create `trace_lines.rs` with A–N as private functions: `ink_mask`, `otsu`,
   `edt`, `zhang_suen`, `skeleton_graph`, `prune`, `pair_junctions`,
   `chain_to_shape`, `enclosed_fills`, `simplify_points`.
3. `editor.rs:7959-7969`:
   - scale strokes, with
     `if let Some(s) = shape.stroke.as_mut() { s.width *= place.determinant().abs().sqrt(); }`;
   - rename `trace_selection` to a public `trace_with(options)`;
   - report the stroke and fill counts in the status line.
4. Command `TraceBitmapOptions`, labelled "Trace Bitmap…", with no shortcut:
   mirror `TraceBitmap` at every `command.rs` site, and put it first in the
   menu (`panels.rs:616`).
5. Editor field `pub trace_dialog: Option<buzz_scene::TraceOptions>`.
   `TraceBitmapOptions` checks that a picture is selected (the same message
   `trace_with` uses), then sets `Some(self.workspace.trace)`.
6. `workspace.rs`: add `#[serde(default)] pub trace: buzz_scene::TraceOptions`.
7. New `buzz-ui/src/trace_dialog.rs`:
   - `pub enum TraceDialogAction { None, Trace, Cancel }`, and
     `pub fn trace_dialog(ctx, &mut TraceOptions) -> TraceDialogAction`.
   - Mode radio: Outline fills / Line art / Both.
   - When the mode isn't Outline:
     - Smoothness slider;
     - Shortest line (0..=100 px);
     - Line style (Plain line / Brush stroke with variable width);
     - Fill enclosed areas;
     - Thickest line (a diameter of 2..=40 px, stored halved).
   - When the mode isn't LineArt: Colours (2..=64), Detail, Corner smoothing,
     Drop background.
   - Always: Speck size.
   - Buttons: Trace and Cancel.
8. `app.rs`, next to the other modal windows:
   - on Trace: copy the options, set `trace_dialog = None`, set
     `workspace.trace = o`, call `workspace.save()`, then `editor.trace_with(o)`;
   - on Cancel: set it to None.

### Tests
`trace_lines.rs` unit tests (helpers `paper`, `ink_line`, `ink_ring`,
`ink_wedge`):
1. A 200×120 line from (20,60) to (180,60) at 5 px (Stroke style): 1 stroke;
   every point has `|y-60| <= 0.75`; the endpoints are within 4 px of the
   ends; the width is 5 ± 1.5.
2. A ring (r=40, t=4): 1 closed path; every point is 40 ± 1.5 from the centre.
3. A plus sign gives 2 strokes.
4. A T gives 2 strokes.
5. A wedge 2→12 px (Brush style): the vertical extent at x=40 is less than
   0.5 × the extent at x=160.
6. 2×2 specks only: 0 strokes.
7. A 100 px square outline (t=4) with `fill_enclosed`: 1 stroke and 1 fill,
   the fill spanning the interior out to the centreline ± 2.
8. A disc of r=20 in LineArt mode: 1 mass fill, 0 strokes.
9. A face (ring, two dashes, an arc): 4–6 strokes.
10. A line with a 2 px bump: 1 stroke.
11. The output is deterministic.

`buzz-app/tests/trace_bitmap.rs`:
- a line PNG traced with `trace_with(LineArt, Stroke)` gives stroked shapes
  with `fill` None, a width of about 4× the pixel width (±25%), in the right
  place; one undo restores the picture;
- `TraceBitmapOptions` sets `trace_dialog` and leaves the document untouched;
- the existing tests still pass.

`workspace.rs`: the `trace` field round-trips, and old JSON still loads.

---
## 4. Break Apart (Ctrl+B), including bitmaps

### Findings
- There is no Break Apart command, and Ctrl+B is free (Ctrl+Shift+B is
  `ToggleBoneRig`, `command.rs:700`).
- Bitmaps are already image-filled shapes (`buzz-scene/src/image.rs:3-18`,
  `FillSpec::image` `object.rs:205`, `Scene::place_image` `lib.rs:749-787`),
  and the Lasso, Wand and Delete already cut them (`tests/bitmap_tools.rs`).
- The gaps:
  - the command itself;
  - bitmaps that arrive inside symbol instances, groups or warps;
  - image shapes that carry an `object.transform`.
- Idioms to reuse:
  - `ungroup_selection` (`editor.rs:7696-7725`);
  - instance expansion as in `buzz-render/src/document.rs:1699-1716`
    (`resolve_frame`, `drawable_at`, skip mask and guide layers,
    `inherited_transform`, `frames.resolved_at`).

### Design
`Editor::break_apart_selection` runs as one `doc.edit("Break Apart", ..)` and
takes one level per press, as Animate does:
- **An image-filled shape:**
  - bake it: `path = T*path`, `fill.transform = T*fill.transform`,
    `object.transform = identity`, `tile = true`;
  - if it is already identity with tiling on, set the status "Already paint:
    cut it with the Lasso, Eraser or Wand".
- **Text** (`object.text`): set `text = None`; the glyph outlines stay.
- **A group:** the `ungroup_selection` logic.
- **An instance:** its symbol's contents at the frame being shown.
  - Each child gets `transform = object.transform * follows * child.transform`
    and a fresh id from `scene.next_object_id()`.
  - A non-identity `instance.color` is baked with `Paint::map_colors`, using
    the renderer's ColorTransform apply.
  - Nested instances stay instances.
  - `elapsed` is computed as `elapsed_on` does.
  - Insert with `scene.add_object`, or with an insert-at-index API if one
    exists, to keep the z-order.
- **A warp:** `ObjectKind::Shape(warp.warped())`.
- **An armature:** skipped, with the status "Unbind the rig before breaking it
  apart".
- **A plain shape:** a no-op with no history step.

Afterwards, select what was produced and set the status "Broke apart N objects".

### Steps
1. `command.rs`: add `BreakApart`, labelled "Break Apart", shortcut
   `sc(ctrl, Key::B)`, in the selection-enabled list and in the ALL lists,
   mirroring `UngroupSelection`.
2. Menus: `panels.rs:535` (after Ungroup), `app.rs:1444` and `app.rs:5061`.
3. `editor.rs:3774`: dispatch to `self.break_apart_selection()`, implemented
   next to `ungroup_selection`.

### Tests
New file `buzz-app/tests/break_apart.rs` (fixture idiom from `trace_bitmap.rs:38-63`):
1. An image shape with a rotate+scale transform, after Break Apart:
   - the transform is identity;
   - the path bounds equal the old world bounds (±1e-6);
   - the fill transform is `T*old`;
   - tiling is on;
   - the wand selects the same bounds ±1.
2. A Lasso cut afterwards gives 2 objects, both `Paint::Image`.
3. An Eraser drag reduces the area and the fill stays `Paint::Image`; fix the
   eraser if it doesn't.
4. An image shape, then Convert to Symbol, then Break Apart: no instance
   remains; the bounds match (±1); one undo restores the instance.
5. A group frees its children.
6. Text: `text` becomes None and the path is unchanged.
7. Ctrl+B maps to `BreakApart`, with no duplicate shortcuts.
8. On a plain shape: a status message and no undo step.

---
## Risks
- Pooling changes how default strokes look.
- A very thin Chisel or Ellipse nib turning sharply can pinch; non-zero fill
  hides the self-overlap.
- `Editor::default()` loads the real workspace, so only the dialog reads
  `workspace.trace`.
- Zhang-Suen on a 6 MP scan runs on the UI thread, bounded by the downsample;
  moving it to `tasks.rs` is a follow-up.
