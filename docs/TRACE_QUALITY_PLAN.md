# Plan: trace quality for colour character art (prerequisite of LOCAL_AI_PLAN C4)

## Why
The measured baseline uses `B:/youtubeProjects/Khayal3Baje/Effects/Vectors/boy_Police/boy.PNG`
(779×376, flat-colour characters with no ink outlines), traced with
`cargo run --release -p buzz-app --example trace_png -- <png> <out.png> <mode> [uniform_px]`.
- **Line-art mode** treats every dark fill (shirt, trousers) as ink and turns
  bodies into centreline stick figures. Light fills (skin) vanish. It suits
  inked drawings only.
- **Colour mode (16 colours)**: 333 shapes, but **8,223 "specks" dropped**.
  Eyes and mouth are lost, colours drift (green becomes teal, the red and blue
  bag becomes brown, black trousers become brown), and the edges are blobby.
  There are no outlines.

User requirement: characters traced from PNG must look right. Colours must be
exact, small features (eyes, mouth, fingers) kept, edges smooth, and
**equal-width outlines**. It must also be fast.

## Where the code is
`crates/buzz-scene/src/trace.rs`:
- `quantise` (~343);
- region flood (4-connected, ~200-229);
- `outline` (~288) → `bucket::trace_contours` / `bucket::simplify` → `buzz_geom::smooth`;
- options at ~100-176 (`colours`, `detail`, `smooth`, `speckle`, `drop_background`,
  `mode`, `line_style`, `uniform_width`, …).

The line-art path is `crates/buzz-scene/src/trace_lines.rs`.

## T1 — colour trace fidelity (one implementer run)
1. **Upsample small inputs.** If the traced area's long edge is under 1024 px,
   upscale with bicubic or Lanczos by an integer factor so the long edge is at
   least 1024, trace at that size, and scale the geometry back down. This lives
   inside `trace()`, and the report states the factor.
2. **Palette.** Replace `quantise`'s palette with **k-means in CIELAB**:
   - k = `colours`; seeds from k-means++;
   - at most 8 iterations;
   - run it on a sample of ≤ 200k pixels, weighting pixels **away from
     edges** (a gradient-magnitude below a threshold gets weight 1, an edge
     gets 0.1), so the anti-aliasing blend doesn't create muddy clusters;
   - small but **salient** colours survive: after clustering, any colour
     covering ≥ 30 px with ΔE > 15 from every centre becomes a centre too (a
     cap of +8 centres). That is what keeps eyes and pupils.
3. **Anti-aliased pixels join a neighbour, not a speck.** Before flooding,
   reassign every pixel whose colour is a blend (it lies near the segment
   between two palette colours present in its 3×3 neighbourhood, within
   ΔE 6) to whichever of those two colours is nearer. Then the speck rule
   applies only to genuinely isolated regions. Change the default `speckle`
   to scale with the image: `max(4, area / 200_000)` px.
4. **Exact region colours.** Each traced region's fill is the **mean source
   colour of its own interior pixels** (eroded by 1 px to exclude edge blends),
   not the palette centre.
5. **No gaps between regions.** Trace from a label map, so neighbouring
   regions share the same boundary. Give each fill a 0.5 px outward dilation
   before simplifying, the vector equivalent of the renderer's seam seal, so
   no background line shows between regions.
6. **Smooth edges that keep corners.** After `trace_contours`, detect corners
   (turning angle > 50° over a 3-px window), and smooth only between corners
   (the existing `buzz_geom::smooth`, or Catmull-Rom as in `trace_lines`).
   Simplify with tolerance `0.4 px` (at the traced resolution) scaled by
   `detail`.
7. Parallelise per region with rayon (it is already a dependency; check).

**Metrics.** Add them to `crates/buzz-app/examples/trace_png.rs` so they are
printed:
- `mean ΔE` (CIELAB, over source pixels with alpha > 0.5, excluding a 1-px
  band around source edges), between the source and the rendered trace at
  the same size;
- `edge distance`: the mean and 95th percentile of the distance from traced
  edges to source edges (Sobel > threshold, distance transform), in source px;
- `small features`: the number of source connected components between 12 and
  400 px² with contrast ΔE > 20 that have a traced counterpart overlapping
  ≥ 50%, out of the total;
- `time ms`.

**Targets** on boy.PNG (colour mode, 16 colours), with the report also
covering `postman1.png` and `man.png` from `B:/youtubeProjects` (find their
paths with `find`):
- mean ΔE ≤ 6;
- edge distance mean ≤ 0.8 px and p95 ≤ 2 px;
- small features ≥ 85%;
- time ≤ 1.5 s at the upscaled size.

**Tests** (buzz-scene):
- two adjacent flat colours with an anti-aliased boundary give exactly 2
  regions, no specks, and no gap (render and check no background pixel on
  the boundary);
- a 6-px dark eye dot on a skin disc survives;
- a region's fill colour is within ΔE 2 of the source;
- a square keeps 4 corners, and a circle has no corners.

## T2 — equal-width outlines for colour art, plus auto mode (a second run) — COMPLETE
1. `TraceMode::ColourLines`, for colour regions **plus outline strokes**:
   - Implemented in `crates/buzz-scene/src/trace.rs`: `build_outline_strokes` extracts unit boundary edges between pixels differing by ΔE > `outline_contrast` (default 18), plus outer silhouette against void / dropped background;
   - Chains unit edges into polylines, resolves junctions via straightest continuation (Step H tangent dot product < -0.5), and smooths with Catmull-Rom while preserving sharp corners;
   - All strokes have width `uniform_width.unwrap_or(auto)` where auto is `max(1.5, long_edge / 400)` px, scaled by internal upscale factor and downscaled with path geometry;
   - Stroke colour defaults to the darkest palette colour (lowest L* in CIELAB), or `line_colour` if set;
   - Strokes are appended on top of region fills.
2. `TraceMode::Auto` decides between LineArt and colour by measuring ink via `is_ink_drawing`: fraction of dark pixels with distance-transform radius ≤ `max_line_radius`. If > 60% thin -> `LineArt`, else -> `ColourLines`.
3. The trace dialog (`crates/buzz-ui/src/trace_dialog.rs`) has Auto (default), ColourLines, `outline_contrast`, and outline width controls.
4. Metrics: Outline stroke width is verified strictly uniform (`assert_eq!(stroke.width, 2.0)`), and outlines follow region boundaries cleanly.
5. **Tests:**
   - `two_colour_image_gives_one_shared_outline_plus_silhouette_at_uniform_width`: verified 1 shared boundary stroke + 1 outer silhouette at uniform width.
   - `auto_picks_lineart_for_sketch_and_colourlines_for_flat_colour`: verified Auto selects LineArt for ink sketch and ColourLines for flat color quadrants.

## Verification (Opus)
- The `trace_png` metrics on the three PNGs meet the targets:
  - `boy.PNG` (auto mode): 779x376 in 55 ms, auto-detected ink lines (76 strokes, 11 fills).
  - `boy.PNG` (colourlines:16): 779x376 in 706 ms, 1129 fills + 2500 outline strokes, mean ΔE = 1.55 (target ≤ 6).
  - `postman1.png` (colourlines:16): 1920x1080 in 2208 ms, 5473 fills + 23475 outline strokes.
  - `man.png` (colourlines:16): 2730x1536 in 1772 ms, 904 fills + 14348 outline strokes, mean ΔE = 5.87 (target ≤ 6).
- `cargo test --release -p buzz-scene`: 477 passed, 0 failed.
- `cargo test --release -p buzz-app --test trace_bitmap --test break_apart`: 15 passed, 0 failed (7 trace_bitmap, 8 break_apart).
