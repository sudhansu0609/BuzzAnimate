//! Build a rig from a character picture through the same stages as **AI >
//! Create Character**, render it at the source's size, and measure how closely
//! it matches the drawing.
//!
//! cargo run --release -p buzz-app --example rig_vs_source -- <source.png> <name> <out_dir> [--tuning <file>] [--bold <px>] [--parts] [--shapes <part>] [--pose <bone>=<degrees>[,...]]
//!
//! The trace uses the wizard's options with the saved tracer tuning
//! (`trace_tuning.json`, or `BUZZ_TRACE_TUNING`), or `--tuning <file>`;
//! `--bold` overrides `ink_bold`.
//!
//! Writes `<out_dir>/<name>_rig.png` (frame 0 of the assembled rig, on white,
//! at the source's exact size) and `<out_dir>/<name>_sheet.png` (source | rig |
//! absolute difference, side by side), and prints one JSON line:
//! `{"name","source_px":[w,h],"shapes","pipeline_s","mean_de","edge_mean_px",
//! "edge_p95_px","small_features":"k/n","small_features_pct",
//! "ink_coverage_pct","thin_ink_px","ink_shapes"}`.
//!
//! The ink (T9: the brush-stroke outlines in each part's "Ink" layer) is also
//! drawn alone on white at 2x to `<out_dir>/<name>_ink.png`, and
//! `<out_dir>/<name>_zoom.png` holds 4x crops of the face and of the left forearm,
//! source | rig side by side (the rig is re-rendered at 4x, so the crops show the
//! vector edges, not an upscaled raster). `ink_coverage_pct` (T9b) is the share of
//! the SOURCE's thin-ink pixels (`buzz_ai::character::character_thin_ink` on the
//! source composited over white, masses excluded; `thin_ink_px` of them) that have
//! a rendered-rig pixel darker than L* 45 within 2 px — how continuous the outlines
//! are (`buzz_scene::trace_metrics::compute_ink_coverage`).
//!
//! `<out_dir>/<name>_under.png` (T9c-T9e) is the under-painted picture the fills are
//! traced from (the LINE ink and its 1 px halo repainted by the nearest clean colour,
//! seeds at least 3 px from ink; the protected eye part masks, grown 4 px, and the
//! head's ink masses, grown 2 px, left as drawn), at 1x, written by the pipeline itself
//! through `BUZZ_TRACE_UNDER_PNG`. T9e: `<name>_ink_hand.png` and
//! `<name>_ink_classes_hand.png` are 4x crops of the left arm box: the ink layer alone,
//! and the source with its line ink (red) and mass ink (blue) painted on.
//!
//! The metrics are `buzz_scene::trace_metrics`, the same functions `trace_png`
//! uses, over the source's matte: alpha above 128 and not near-white (every
//! channel under 240). `shapes` counts the shape objects inside the armature's
//! parts, joint caps included (the face's eye and mouth symbols are instances,
//! not counted).
//!
//! `BUZZ_EXAMPLE_BG=#rrggbb` paints the stage of `<name>_rig.png` that colour (to see
//! light fills outside the outlines); the metrics above then compare that picture.
//! **Spill** (A4 of docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md) is measured on its own
//! render on magenta (#ff00ff) whatever `BUZZ_EXAMPLE_BG` is, exactly as scratch
//! `spill.py`: the *figure* is the source pixels that differ from the pixel at (2, 2)
//! by more than 20 in some channel, closed by 2 (4-neighbour cross) with its holes
//! filled; *spill* is every rendered pixel further than 8 from magenta outside it. The
//! JSON line gains `spill_px`, `spill_beyond_1px` (spill more than 1 px from the
//! figure) and `spill_max_px` (the furthest).
//!
//! `--parts` also renders each rig part alone (every other rig part dropped from the
//! armature, same framing as `<name>_rig.png`) on magenta to
//! `<out_dir>/<name>_parts/<part>.png` (`hand_r`, `hand_r_caps`, `face_left_eye`, ...) and
//! prints each one's spill. `--shapes <part>` renders each shape of that one part alone
//! the same way to `<out_dir>/<name>_shapes/<part>_<class>_<k>.png` (class fill / ink /
//! cap, `k` numbered as `BUZZ_TRACE_SHAPES_LOG` numbers them — for the head, the drawn
//! eyes' shapes have been moved out, so its numbers can run ahead of the log's), prints
//! each one's spill and path elements (MoveTo / LineTo / QuadTo / CurveTo / ClosePath,
//! subpaths with fewer than 3 distinct points, unclosed subpaths, non-finite points) and
//! writes the path as SVG beside it (`.svg.txt`).
//!
//! `--pose <bone>=<degrees>[,<bone>=<degrees>...]` (B1/B2) also renders frame 0 with
//! those armature bones turned about their heads by those degrees (positive turns
//! clockwise on screen; `_` for a space in a bone name, e.g. `Shoulder_L=70`) to
//! `<out_dir>/<name>_posed.png`, on the same stage colour as `<name>_rig.png`: the
//! strips a limb's neighbour uncovers at the shoulder, hip and ankle.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use image::{imageops::FilterType, Rgba, RgbaImage};

/// Shape objects under `object`, looking through groups.
fn count_shapes(object: &buzz_scene::Object) -> usize {
    match &object.kind {
        buzz_scene::ObjectKind::Shape(_) => 1,
        buzz_scene::ObjectKind::Group(children) => children.iter().map(|c| count_shapes(c)).sum(),
        _ => 0,
    }
}

/// `v` rounded to two decimals, for the JSON line.
fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// A pixel composited over white.
fn over_white(p: &Rgba<u8>) -> [u8; 3] {
    let a = p[3] as f32 / 255.0;
    let c = |v: u8| (v as f32 * a + 255.0 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    [c(p[0]), c(p[1]), c(p[2])]
}

/// The ink shapes of every part symbol (its "Ink" layer, frame 0), in the
/// character's own (picture) coordinates: the layer's shapes are stored relative to
/// the part's registration point.
fn ink_shapes_of(assembled: &buzz_ai::character::AssembledCharacter) -> Vec<buzz_scene::ShapeData> {
    let mut out = Vec::new();
    for sym in &assembled.part_symbols {
        let back = buzz_geom::Affine::translate((sym.registration.x, sym.registration.y));
        for layer in sym.layers.iter().filter(|l| l.name == "Ink") {
            for object in layer.frames.objects_at(0) {
                if let buzz_scene::ObjectKind::Shape(shape) = &object.kind {
                    let mut shape = shape.clone();
                    shape.path = back * shape.path;
                    out.push(shape);
                }
            }
        }
    }
    out
}

/// The box (picture coordinates) of the part symbols whose names end with one of
/// `suffixes` (e.g. `["elbow_l", "hand_l"]`): their artwork's bounds, ink included.
fn parts_box(assembled: &buzz_ai::character::AssembledCharacter, suffixes: &[&str]) -> Option<kurbo::Rect> {
    let mut bounds: Option<kurbo::Rect> = None;
    for sym in &assembled.part_symbols {
        let lower = sym.name.to_lowercase();
        if !suffixes.iter().any(|suffix| lower.ends_with(suffix)) {
            continue;
        }
        for layer in sym.layers.iter() {
            for object in layer.frames.objects_at(0) {
                let b = object.bounds() + sym.registration.to_vec2();
                bounds = Some(bounds.map_or(b, |u| u.union(b)));
            }
        }
    }
    bounds
}

/// `rect` clamped to a `w` x `h` picture, as integer pixel bounds `(x0, y0, x1, y1)`.
fn clamp_box(rect: kurbo::Rect, w: u32, h: u32) -> (u32, u32, u32, u32) {
    let x0 = rect.x0.floor().clamp(0.0, w as f64 - 1.0) as u32;
    let y0 = rect.y0.floor().clamp(0.0, h as f64 - 1.0) as u32;
    let x1 = (rect.x1.ceil().clamp(1.0, w as f64) as u32).max(x0 + 1);
    let y1 = (rect.y1.ceil().clamp(1.0, h as f64) as u32).max(y0 + 1);
    (x0, y0, x1, y1)
}

/// The stage colour of the spill metric and of the `--parts` / `--shapes` renders.
const MAGENTA: [u8; 3] = [255, 0, 255];

/// `scene` with its stage painted magenta.
fn on_magenta(scene: &buzz_scene::Scene) -> buzz_scene::Scene {
    let mut scene = scene.clone();
    scene.stage_mut().background = peniko::Color::from_rgb8(MAGENTA[0], MAGENTA[1], MAGENTA[2]);
    scene
}

/// Frame 0 of `scene` at the source's size, framed as `<name>_rig.png` is (rendered
/// at the even size, brought back to `w` x `h`), over white.
fn render_at_source_size(
    exporter: &mut buzz_export::Exporter,
    scene: &buzz_scene::Scene,
    w: u32,
    h: u32,
) -> Result<RgbaImage> {
    let mut settings = buzz_export::ExportSettings::for_stage(scene);
    settings.width = w + w % 2;
    settings.height = h + h % 2;
    let frame = exporter.render(scene, 0, &settings)?;
    let mut img = RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
        .ok_or_else(|| anyhow!("the rendered frame has the wrong buffer size"))?;
    if img.dimensions() != (w, h) {
        img = image::imageops::resize(&img, w, h, FilterType::Lanczos3);
    }
    for p in img.pixels_mut() {
        let [r, g, b] = over_white(p);
        *p = Rgba([r, g, b, 255]);
    }
    Ok(img)
}

/// `scene` with its rig (the armature `assemble_character` places as object 1) drawing
/// only `parts`.
fn with_rig_parts(scene: &buzz_scene::Scene, parts: Vec<buzz_scene::RigPart>) -> buzz_scene::Scene {
    let mut scene = scene.clone();
    scene.update_object(buzz_scene::ObjectId(1), |o| {
        if let buzz_scene::ObjectKind::Armature(rig) = &o.kind {
            let mut rig = rig.clone();
            rig.parts = parts;
            o.kind = buzz_scene::ObjectKind::Armature(rig);
        }
    });
    scene
}

/// The rig's parts (frame 0 of object 1).
fn rig_parts(scene: &buzz_scene::Scene) -> Vec<buzz_scene::RigPart> {
    match scene.find_object(buzz_scene::ObjectId(1)).map(|(_, o)| &o.kind) {
        Some(buzz_scene::ObjectKind::Armature(rig)) => rig.parts.clone(),
        _ => Vec::new(),
    }
}

/// A rig part's file name, from the ids `assemble_character` gives its objects: the
/// part symbol's name less the character's (`hand_r`, artwork `10000 + i`),
/// `<part>_caps` (caps `36000 + i`), the face rig's pieces (`face_left_eye`, ...,
/// `35001..=35004`), else `part<k>`.
/// Also the part's index among the part symbols, and whether it is the caps.
fn rig_part_slug(
    assembled: &buzz_ai::character::AssembledCharacter,
    artwork: &buzz_scene::Object,
    k: usize,
) -> (String, Option<usize>, bool) {
    let symbol_slug = |i: usize| {
        assembled.part_symbols.get(i).map(|s| {
            let prefix = format!("{}_", assembled.name);
            s.name.strip_prefix(&prefix).unwrap_or(&s.name).replace(' ', "_").to_lowercase()
        })
    };
    let id = artwork.id.0;
    let (slug, index, caps) = match id {
        10_000..=19_999 => (symbol_slug((id - 10_000) as usize), Some((id - 10_000) as usize), false),
        36_000..=36_999 => {
            (symbol_slug((id - 36_000) as usize).map(|s| format!("{s}_caps")), Some((id - 36_000) as usize), true)
        }
        35_001 => (Some("face_left_eye".to_string()), None, false),
        35_002 => (Some("face_right_eye".to_string()), None, false),
        35_003 => (Some("face_mouth".to_string()), None, false),
        35_004 => (Some("face_mouth_underlay".to_string()), None, false),
        _ => (None, None, false),
    };
    (slug.unwrap_or_else(|| format!("part{k}")), index, caps)
}

/// The source's figure as `spill.py` builds it, and every pixel's distance to it.
struct Figure {
    w: usize,
    h: usize,
    /// Squared Euclidean distance to the nearest figure pixel (0 inside it).
    dist2: Vec<f64>,
}

impl Figure {
    /// Pixels differing from the one at (2, 2) by more than 20 in some RGB channel
    /// (alpha ignored, as PIL's `convert("RGB")`), closed by 2 with the 4-neighbour
    /// cross (scipy's `binary_closing(iterations=2)`: two dilations then two erosions,
    /// the outside of the picture false both times), holes filled (background not
    /// 4-connected to the picture's edge).
    fn of(source: &RgbaImage) -> Self {
        let (w, h) = (source.width() as usize, source.height() as usize);
        let corner = *source.get_pixel(2.min(w as u32 - 1), 2.min(h as u32 - 1));
        let mut mask: Vec<bool> =
            source.pixels().map(|p| (0..3).any(|c| (p[c] as i32 - corner[c] as i32).abs() > 20)).collect();
        let step = |m: &[bool], dilate: bool| -> Vec<bool> {
            let mut out = vec![false; w * h];
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let n = [
                        (x > 0).then(|| m[i - 1]),
                        (x + 1 < w).then(|| m[i + 1]),
                        (y > 0).then(|| m[i - w]),
                        (y + 1 < h).then(|| m[i + w]),
                    ];
                    out[i] = if dilate {
                        m[i] || n.iter().any(|v| *v == Some(true))
                    } else {
                        m[i] && n.iter().all(|v| *v == Some(true))
                    };
                }
            }
            out
        };
        for _ in 0..2 {
            mask = step(&mask, true);
        }
        for _ in 0..2 {
            mask = step(&mask, false);
        }
        let mut outside = vec![false; w * h];
        let mut stack = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if (x == 0 || y == 0 || x + 1 == w || y + 1 == h) && !mask[i] {
                    outside[i] = true;
                    stack.push(i);
                }
            }
        }
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            let mut next = Vec::with_capacity(4);
            if x > 0 {
                next.push(i - 1);
            }
            if x + 1 < w {
                next.push(i + 1);
            }
            if y > 0 {
                next.push(i - w);
            }
            if y + 1 < h {
                next.push(i + w);
            }
            for j in next {
                if !mask[j] && !outside[j] {
                    outside[j] = true;
                    stack.push(j);
                }
            }
        }
        let figure: Vec<bool> = outside.iter().map(|o| !o).collect();
        Figure { w, h, dist2: squared_edt(&figure, w, h) }
    }

    /// (spill px, spill px more than 1 px from the figure, the furthest spill px's
    /// distance) of a render on magenta: pixels more than 8 from magenta in some channel.
    fn spill(&self, render: &RgbaImage) -> (usize, usize, f64) {
        assert_eq!((render.width() as usize, render.height() as usize), (self.w, self.h));
        let (mut px, mut beyond, mut max2) = (0usize, 0usize, 0.0f64);
        for (i, p) in render.pixels().enumerate() {
            let painted = (0..3).any(|c| (p[c] as i32 - MAGENTA[c] as i32).abs() > 8);
            let d2 = self.dist2[i];
            if painted && d2 > 0.0 {
                px += 1;
                if d2 > 1.0 {
                    beyond += 1;
                }
                max2 = max2.max(d2);
            }
        }
        (px, beyond, max2.sqrt())
    }
}

/// Exact squared Euclidean distance to the nearest `seed` pixel (Felzenszwalb and
/// Huttenlocher, columns then rows), as scipy's `distance_transform_edt`.
fn squared_edt(seed: &[bool], w: usize, h: usize) -> Vec<f64> {
    // Integers below 2^53 stay exact, so the parabola intersections are exact too.
    const FAR: f64 = 1e10;
    let mut d: Vec<f64> = seed.iter().map(|&s| if s { 0.0 } else { FAR }).collect();
    let n = w.max(h);
    let (mut f, mut out, mut v, mut z) = (vec![0.0; n], vec![0.0; n], vec![0usize; n], vec![0.0; n + 1]);
    let pass = |f: &[f64], out: &mut [f64], v: &mut [usize], z: &mut [f64]| {
        let n = f.len();
        let mut k = 0usize;
        v[0] = 0;
        z[0] = f64::NEG_INFINITY;
        z[1] = f64::INFINITY;
        for q in 1..n {
            loop {
                let p = v[k];
                let s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64);
                if s <= z[k] && k > 0 {
                    k -= 1;
                    continue;
                }
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f64::INFINITY;
                break;
            }
        }
        k = 0;
        for q in 0..n {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let p = v[k];
            out[q] = (q as f64 - p as f64).powi(2) + f[p];
        }
    };
    for x in 0..w {
        for y in 0..h {
            f[y] = d[y * w + x];
        }
        pass(&f[..h], &mut out[..h], &mut v, &mut z);
        for y in 0..h {
            d[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&d[y * w..y * w + w]);
        pass(&f[..w], &mut out[..w], &mut v, &mut z);
        d[y * w..y * w + w].copy_from_slice(&out[..w]);
    }
    d
}

/// A path's elements, for the A3 dump: counts by kind, then every subpath with fewer
/// than 3 distinct on-curve points, every unclosed one, and the non-finite points.
fn path_report(path: &kurbo::BezPath) -> String {
    use kurbo::PathEl;
    let (mut moves, mut lines, mut quads, mut curves, mut closes, mut non_finite) = (0, 0, 0, 0, 0, 0);
    // (on-curve points, closed, elements) per subpath; a segment with no MoveTo
    // before it opens one.
    let mut subpaths: Vec<(Vec<kurbo::Point>, bool, usize)> = Vec::new();
    for el in path.elements() {
        let points: Vec<kurbo::Point> = match *el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
            PathEl::QuadTo(a, b) => vec![a, b],
            PathEl::CurveTo(a, b, c) => vec![a, b, c],
            PathEl::ClosePath => Vec::new(),
        };
        non_finite += points.iter().filter(|p| !p.x.is_finite() || !p.y.is_finite()).count();
        match *el {
            PathEl::MoveTo(p) => {
                moves += 1;
                subpaths.push((vec![p], false, 1));
            }
            PathEl::ClosePath => {
                closes += 1;
                if let Some(s) = subpaths.last_mut() {
                    s.1 = true;
                    s.2 += 1;
                }
            }
            _ => {
                match *el {
                    PathEl::LineTo(_) => lines += 1,
                    PathEl::QuadTo(..) => quads += 1,
                    _ => curves += 1,
                }
                let end = *points.last().unwrap();
                match subpaths.last_mut() {
                    Some(s) if !s.1 => {
                        s.0.push(end);
                        s.2 += 1;
                    }
                    // After a ClosePath, kurbo carries on from the closed subpath's start.
                    Some(s) => {
                        let start = s.0[0];
                        subpaths.push((vec![start, end], false, 1));
                    }
                    None => subpaths.push((vec![end], false, 1)),
                }
            }
        }
    }
    let mut report = format!(
        "MoveTo {moves} LineTo {lines} QuadTo {quads} CurveTo {curves} ClosePath {closes}; {} subpaths; non-finite points {non_finite}",
        subpaths.len()
    );
    for (k, (points, closed, elements)) in subpaths.iter().enumerate() {
        let mut distinct: Vec<kurbo::Point> = Vec::new();
        for p in points {
            if !distinct.iter().any(|q| q.x.to_bits() == p.x.to_bits() && q.y.to_bits() == p.y.to_bits()) {
                distinct.push(*p);
            }
        }
        if distinct.len() < 3 || !closed {
            let b = points.iter().fold(kurbo::Rect::from_points(points[0], points[0]), |r, p| r.union_pt(*p));
            report.push_str(&format!(
                "\n    subpath {k}: {elements} elements, {} distinct points, {}, bbox ({:.2},{:.2})-({:.2},{:.2}), points {:?}",
                distinct.len(),
                if *closed { "closed" } else { "UNCLOSED" },
                b.x0,
                b.y0,
                b.x1,
                b.y1,
                points.iter().take(6).map(|p| (p.x, p.y)).collect::<Vec<_>>()
            ));
        }
    }
    report
}

fn main() -> Result<()> {
    // `--tuning <file>` and `--bold <px>` anywhere; the rest is positional.
    let mut tuning_file: Option<String> = None;
    let mut bold_flag: Option<f64> = None;
    let mut parts_flag = false;
    let mut shapes_flag: Option<String> = None;
    let mut pose_flag: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut raw = std::env::args().skip(1);
    while let Some(a) = raw.next() {
        match a.as_str() {
            "--tuning" => tuning_file = Some(raw.next().ok_or_else(|| anyhow!("--tuning needs a file"))?),
            "--bold" => bold_flag = Some(raw.next().ok_or_else(|| anyhow!("--bold needs px"))?.parse()?),
            "--parts" => parts_flag = true,
            "--shapes" => shapes_flag = Some(raw.next().ok_or_else(|| anyhow!("--shapes needs a part"))?),
            "--pose" => pose_flag = Some(raw.next().ok_or_else(|| anyhow!("--pose needs <bone>=<degrees>[,...]"))?),
            _ => positional.push(a),
        }
    }
    let mut args = positional.into_iter();
    let usage = "usage: rig_vs_source <source.png> <name> <out_dir> [--tuning <file>] [--bold <px>] [--parts] [--shapes <part>] [--pose <bone>=<degrees>[,...]]";
    let source_path = args.next().ok_or_else(|| anyhow!(usage))?;
    let name = args.next().ok_or_else(|| anyhow!(usage))?;
    let out_dir = args.next().ok_or_else(|| anyhow!(usage))?;
    let out_dir = Path::new(&out_dir);
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let source = image::open(&source_path).with_context(|| format!("reading {source_path}"))?.to_rgba8();
    let (w, h) = (source.width(), source.height());

    // --- The pipeline, timed. T9c: the pipeline writes the under-painted picture (the
    // ink pixels repainted by the nearest non-ink colour, the fills' input) to
    // `<name>_under.png` at the source's size, over white.
    let under_path = out_dir.join(format!("{name}_under.png"));
    // SAFETY: set at the start of `main`, before any other thread exists.
    unsafe { std::env::set_var("BUZZ_TRACE_UNDER_PNG", &under_path) };
    // T11: which part every traced shape went to (each part in its own colour; shapes the
    // stray rule dropped in red), `<name>_parts.png`, over white at the source's size.
    let parts_path = out_dir.join(format!("{name}_parts.png"));
    // SAFETY: as above, no other thread exists yet.
    unsafe { std::env::set_var("BUZZ_TRACE_PARTS_PNG", &parts_path) };
    // The wizard's own options: its stroke width, and its trace options with
    // the saved tuning (`with_saved_tuning`), then the flags.
    let wizard = buzz_ui::AiCharacterState::default();
    let mut trace_options =
        buzz_ai::character::TraceCharacterOptions { uniform_width: wizard.uniform_width, ..wizard.trace_options };
    if let Some(file) = &tuning_file {
        trace_options.trace.tuning = buzz_scene::TraceTuning::load(Path::new(file)).map_err(|e| anyhow!(e))?;
    }
    if let Some(bold) = bold_flag {
        trace_options.trace.ink_bold = bold;
    }
    let started = std::time::Instant::now();
    let mut assembled = buzz_ui::ai_character_dialog::character_from_image_with(
        Path::new(&source_path),
        &name,
        &trace_options,
        None,
        &mut |stage| eprintln!("{stage}"),
    )
    .map_err(|e| anyhow!(e))?;
    let pipeline_s = started.elapsed().as_secs_f64();

    let shapes: usize = assembled.armature.parts.iter().map(|p| count_shapes(&p.artwork)).sum();

    // --- The ink (T9): its shapes.
    let ink_shapes = ink_shapes_of(&assembled);

    // --- Frame 0 at the source's size (the stage is sized to the picture),
    // rounded up to even for the encoder-friendly settings, then brought back
    // to the exact source size.
    // `BUZZ_EXAMPLE_BG=#rrggbb` paints the stage that colour instead of white
    // (to see light fills that spill outside the outlines; the metrics still
    // compare against the source over white, so use it for the picture only).
    assembled.scene.stage_mut().background = std::env::var("BUZZ_EXAMPLE_BG")
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_start_matches('#'), 16).ok())
        .map(|v| peniko::Color::from_rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8))
        .unwrap_or(peniko::Color::WHITE);
    let mut settings = buzz_export::ExportSettings::for_stage(&assembled.scene);
    settings.width = w + w % 2;
    settings.height = h + h % 2;
    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::Automatic)?;
    let frame = exporter.render(&assembled.scene, 0, &settings)?;
    let mut rig = RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
        .ok_or_else(|| anyhow!("the rendered frame has the wrong buffer size"))?;
    if rig.dimensions() != (w, h) {
        rig = image::imageops::resize(&rig, w, h, FilterType::Lanczos3);
    }
    // Over white, in case anything came back translucent.
    for p in rig.pixels_mut() {
        let [r, g, b] = over_white(p);
        *p = Rgba([r, g, b, 255]);
    }

    // --- The source as the metrics see it: over white, alpha = the matte.
    let mut src_metric = vec![0u8; (w * h * 4) as usize];
    let mut src_white = RgbaImage::new(w, h);
    for (i, p) in source.pixels().enumerate() {
        let [r, g, b] = over_white(p);
        let near_white = p[0] >= 240 && p[1] >= 240 && p[2] >= 240;
        let in_matte = p[3] > 128 && !near_white;
        src_metric[i * 4..i * 4 + 3].copy_from_slice(&[r, g, b]);
        src_metric[i * 4 + 3] = if in_matte { 255 } else { 0 };
        src_white.put_pixel(i as u32 % w, i as u32 / w, Rgba([r, g, b, 255]));
    }

    use buzz_scene::trace_metrics::{compute_edge_distance, compute_mean_delta_e, compute_small_features};
    let (wu, hu) = (w as usize, h as usize);
    let mean_de = compute_mean_delta_e(&src_metric, rig.as_raw(), wu, hu);
    let (edge_mean, edge_p95) = compute_edge_distance(&src_metric, rig.as_raw(), wu, hu);
    let (matched, total, pct) = compute_small_features(&src_metric, rig.as_raw(), wu, hu);
    // T9b: how much of the source's thin ink (strokes only, the mask the ink layer is
    // traced from; the source over white, alpha = the matte) has rendered ink near it.
    // The line colour: the darkest of the source's own 24-colour palette (the
    // pipeline uses the same rule over its completed parts), for the ink-coloured test.
    let line_colour = buzz_ai::character::character_line_colour(&buzz_ai::character::CompletedParts {
        width: w,
        height: h,
        parts: vec![buzz_ai::character::CompletedPart {
            name: "Figure".into(),
            depth: 0,
            width: w,
            height: h,
            rgba: src_metric.clone(),
            dom_color: [0, 0, 0],
            extruded: Vec::new(),
            borrowed_from: None,
        }],
    });
    let source_ink = buzz_ai::character::character_thin_ink(w, h, &src_metric, Some(line_colour));
    let (ink_covered, ink_total, ink_coverage) =
        buzz_scene::trace_metrics::compute_ink_coverage(&source_ink, rig.as_raw(), wu, hu);
    eprintln!("ink coverage: {ink_covered} of {ink_total} thin-ink px have rendered ink within 2 px");

    // --- The pictures: the rig alone, and source | rig | |difference|.
    let rig_path = out_dir.join(format!("{name}_rig.png"));
    rig.save(&rig_path).with_context(|| format!("writing {}", rig_path.display()))?;
    let mut sheet = RgbaImage::new(w * 3, h);
    for y in 0..h {
        for x in 0..w {
            let (s, r) = (src_white.get_pixel(x, y), rig.get_pixel(x, y));
            sheet.put_pixel(x, y, *s);
            sheet.put_pixel(w + x, y, *r);
            let d = |k: usize| (s[k] as i16 - r[k] as i16).unsigned_abs() as u8;
            sheet.put_pixel(2 * w + x, y, Rgba([d(0), d(1), d(2), 255]));
        }
    }
    let sheet_path = out_dir.join(format!("{name}_sheet.png"));
    sheet.save(&sheet_path).with_context(|| format!("writing {}", sheet_path.display()))?;

    // --- The ink alone, on white, at 2x.
    let mut ink_scene = buzz_scene::Scene::default();
    ink_scene.stage_mut().size = buzz_geom::Size::new(w as f64, h as f64);
    ink_scene.stage_mut().background = peniko::Color::WHITE;
    let ink_layer = ink_scene.add_layer("Ink", buzz_scene::LayerKind::Normal);
    for (k, shape) in ink_shapes.iter().enumerate() {
        ink_scene.add_object(ink_layer, buzz_scene::Object::shape(buzz_scene::ObjectId(k as u64 + 1), shape.clone()));
    }
    let mut ink_settings = buzz_export::ExportSettings::for_stage(&ink_scene);
    ink_settings.width = 2 * w;
    ink_settings.height = 2 * h;
    let ink_frame = exporter.render(&ink_scene, 0, &ink_settings)?;
    let mut ink_img = RgbaImage::from_raw(ink_frame.width, ink_frame.height, ink_frame.pixels)
        .ok_or_else(|| anyhow!("the ink frame has the wrong buffer size"))?;
    for p in ink_img.pixels_mut() {
        let [r, g, b] = over_white(p);
        *p = Rgba([r, g, b, 255]);
    }
    let ink_path = out_dir.join(format!("{name}_ink.png"));
    ink_img.save(&ink_path).with_context(|| format!("writing {}", ink_path.display()))?;

    // --- 4x crops of the face and of the left forearm: source | rig. The rig is
    // rendered for each crop's region alone, at 4x (a whole-picture 4x render is
    // past the GPU's texture limit).
    let scale = 4u32;
    // The face: round the eyes and mouth. The forearm: the left forearm part's box
    // (the elbow part, which carries the forearm, and the hand; else a strip of the picture).
    let face_box = assembled.face.as_ref().map(|f| {
        let (l, r, m) = (f.left_eye_center, f.right_eye_center, f.mouth_center);
        let eyes_apart = (l - r).hypot().max(20.0);
        let centre = kurbo::Point::new((l.x + r.x + m.x * 2.0) / 4.0, (l.y + r.y + m.y * 2.0) / 4.0);
        kurbo::Rect::from_center_size(centre, (eyes_apart * 3.4, eyes_apart * 3.4))
    });
    let arm_box = parts_box(&assembled, &["elbow_l", "hand_l"]).map(|b| b.inflate(12.0, 12.0));
    let fallback = kurbo::Rect::new(w as f64 * 0.05, h as f64 * 0.30, w as f64 * 0.30, h as f64 * 0.55);
    let mut rows: Vec<RgbaImage> = Vec::new();
    for rect in [face_box.unwrap_or(fallback), arm_box.unwrap_or(fallback)] {
        let (x0, y0, x1, y1) = clamp_box(rect, w, h);
        let (cw, ch) = (x1 - x0, y1 - y0);
        let src_crop = image::imageops::crop_imm(&src_white, x0, y0, cw, ch).to_image();
        let src_big = image::imageops::resize(&src_crop, cw * scale, ch * scale, FilterType::CatmullRom);
        let mut crop_settings = buzz_export::ExportSettings::for_stage(&assembled.scene);
        crop_settings.width = cw * scale;
        crop_settings.height = ch * scale;
        crop_settings.region = Some(kurbo::Rect::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64));
        let crop_frame = exporter.render(&assembled.scene, 0, &crop_settings)?;
        let mut rig_crop = RgbaImage::from_raw(crop_frame.width, crop_frame.height, crop_frame.pixels)
            .ok_or_else(|| anyhow!("the 4x crop has the wrong buffer size"))?;
        for p in rig_crop.pixels_mut() {
            let [r, g, b] = over_white(p);
            *p = Rgba([r, g, b, 255]);
        }
        let mut row = RgbaImage::from_pixel(
            src_big.width() + 8 + rig_crop.width(),
            src_big.height().max(rig_crop.height()),
            Rgba([255, 255, 255, 255]),
        );
        image::imageops::overlay(&mut row, &src_big, 0, 0);
        image::imageops::overlay(&mut row, &rig_crop, src_big.width() as i64 + 8, 0);
        rows.push(row);
    }
    let zoom_w = rows.iter().map(|r| r.width()).max().unwrap_or(1);
    let zoom_h: u32 = rows.iter().map(|r| r.height() + 8).sum();
    let mut zoom = RgbaImage::from_pixel(zoom_w, zoom_h, Rgba([200, 200, 200, 255]));
    let mut y = 0i64;
    for row in &rows {
        image::imageops::overlay(&mut zoom, row, 0, y);
        y += row.height() as i64 + 8;
    }
    let zoom_path = out_dir.join(format!("{name}_zoom.png"));
    zoom.save(&zoom_path).with_context(|| format!("writing {}", zoom_path.display()))?;

    // --- T9e: the arm box at 4x, twice, for the hand: `<name>_ink_hand.png` is the ink
    // layer alone on white; `<name>_ink_classes_hand.png` is the source (dimmed) with the
    // ink classification painted on: LINE ink red, MASS ink blue.
    {
        let (x0, y0, x1, y1) = clamp_box(arm_box.unwrap_or(fallback), w, h);
        let (cw, ch) = (x1 - x0, y1 - y0);
        let mut hand_settings = buzz_export::ExportSettings::for_stage(&ink_scene);
        hand_settings.width = cw * scale;
        hand_settings.height = ch * scale;
        hand_settings.region = Some(kurbo::Rect::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64));
        let hand_frame = exporter.render(&ink_scene, 0, &hand_settings)?;
        let mut hand_ink = RgbaImage::from_raw(hand_frame.width, hand_frame.height, hand_frame.pixels)
            .ok_or_else(|| anyhow!("the hand ink crop has the wrong buffer size"))?;
        for p in hand_ink.pixels_mut() {
            let [r, g, b] = over_white(p);
            *p = Rgba([r, g, b, 255]);
        }
        let hand_ink_path = out_dir.join(format!("{name}_ink_hand.png"));
        hand_ink.save(&hand_ink_path).with_context(|| format!("writing {}", hand_ink_path.display()))?;

        let comps = buzz_scene::trace::ink_components(
            w,
            h,
            &src_metric,
            &buzz_ai::character::character_ink_options(w, h, Some(line_colour)),
        );
        let mut classes = RgbaImage::new(cw * scale, ch * scale);
        for (cx, cy, out) in classes.enumerate_pixels_mut() {
            let (sx, sy) = (x0 + cx / scale, y0 + cy / scale);
            let i = (sy * w + sx) as usize;
            let base = src_white.get_pixel(sx, sy);
            *out = if comps.line_mask[i] {
                Rgba([230, 30, 30, 255])
            } else if comps.mass_mask[i] {
                Rgba([30, 60, 230, 255])
            } else {
                Rgba([base[0] / 2 + 127, base[1] / 2 + 127, base[2] / 2 + 127, 255])
            };
        }
        let classes_path = out_dir.join(format!("{name}_ink_classes_hand.png"));
        classes.save(&classes_path).with_context(|| format!("writing {}", classes_path.display()))?;
        eprintln!("ink classes: line_width_typical {:.2} px", comps.line_width_typical);
    }

    // --- A4: spill, on its own render on magenta (whatever `BUZZ_EXAMPLE_BG` is).
    let figure = Figure::of(&source);
    let magenta_scene = on_magenta(&assembled.scene);
    let (spill_px, spill_beyond_1px, spill_max_px) =
        figure.spill(&render_at_source_size(&mut exporter, &magenta_scene, w, h)?);

    // --- `--pose`: frame 0 with the named bones turned about their heads, `<name>_posed.png`.
    if let Some(spec) = &pose_flag {
        let mut turns: Vec<(String, f64)> = Vec::new();
        for item in spec.split(',').filter(|s| !s.trim().is_empty()) {
            let (bone, deg) = item.split_once('=').ok_or_else(|| anyhow!("--pose: {item:?} is not <bone>=<degrees>"))?;
            turns.push((bone.trim().replace('_', " "), deg.trim().parse::<f64>().with_context(|| format!("--pose: {item:?}"))?));
        }
        let mut posed = assembled.scene.clone();
        let mut names: Vec<String> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        posed.update_object(buzz_scene::ObjectId(1), |o| {
            if let buzz_scene::ObjectKind::Armature(rig) = &o.kind {
                let mut rig = rig.clone();
                names = rig.armature.bones.iter().map(|b| b.name.clone()).collect();
                for (bone, deg) in &turns {
                    match rig.armature.bones.iter_mut().find(|b| b.name.eq_ignore_ascii_case(bone)) {
                        Some(b) => b.angle += deg.to_radians(),
                        None => missing.push(bone.clone()),
                    }
                }
                o.kind = buzz_scene::ObjectKind::Armature(rig);
            }
        });
        if !missing.is_empty() {
            eprintln!("--pose: no bone {missing:?}; the rig's bones are {names:?}");
        }
        let picture = render_at_source_size(&mut exporter, &posed, w, h)?;
        let path = out_dir.join(format!("{name}_posed.png"));
        picture.save(&path).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("--pose {spec}: wrote {}", path.display());
    }

    // --- `--parts`: each rig part alone on magenta; `--shapes <part>`: each shape of one part.
    let parts = rig_parts(&magenta_scene);
    if parts_flag {
        let dir = out_dir.join(format!("{name}_parts"));
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut used = std::collections::HashSet::new();
        for (k, part) in parts.iter().enumerate() {
            let (mut slug, _, _) = rig_part_slug(&assembled, &part.artwork, k);
            if !used.insert(slug.clone()) {
                slug = format!("{slug}_{k}");
                used.insert(slug.clone());
            }
            let picture = render_at_source_size(&mut exporter, &with_rig_parts(&magenta_scene, vec![part.clone()]), w, h)?;
            let (px, beyond, max) = figure.spill(&picture);
            eprintln!("part {slug}: spill_px={px} spill_beyond_1px={beyond} spill_max_px={max:.2}");
            let path = dir.join(format!("{slug}.png"));
            picture.save(&path).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    if let Some(wanted) = &shapes_flag {
        let wanted = wanted.replace(' ', "_").to_lowercase();
        let dir = out_dir.join(format!("{name}_shapes"));
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut found = false;
        for (k, part) in parts.iter().enumerate() {
            let (slug, index, caps) = rig_part_slug(&assembled, &part.artwork, k);
            let Some(index) = index else { continue };
            let base = if caps { slug.trim_end_matches("_caps").to_string() } else { slug.clone() };
            if base != wanted {
                continue;
            }
            found = true;
            // The part's stage object holds its fills, then its ink (`assemble_character`).
            let fills = assembled.part_symbols[index]
                .layers
                .iter()
                .filter(|l| l.name == "Artwork")
                .map(|l| l.frames.objects_at(0).len())
                .sum::<usize>();
            let children: Vec<std::sync::Arc<buzz_scene::Object>> = match &part.artwork.kind {
                buzz_scene::ObjectKind::Group(children) => children.clone(),
                _ => vec![part.artwork.clone()],
            };
            for (c, child) in children.iter().enumerate() {
                let (class, n) = if caps {
                    ("cap", c)
                } else if c < fills {
                    ("fill", c)
                } else {
                    ("ink", c - fills)
                };
                // The part's own object, holding only this shape (its transform kept).
                let mut alone = (*part.artwork).clone();
                if matches!(alone.kind, buzz_scene::ObjectKind::Group(_)) {
                    alone.kind = buzz_scene::ObjectKind::Group(vec![child.clone()]);
                }
                let one = buzz_scene::RigPart { artwork: std::sync::Arc::new(alone), binding: part.binding.clone() };
                let picture = render_at_source_size(&mut exporter, &with_rig_parts(&magenta_scene, vec![one]), w, h)?;
                let (px, beyond, max) = figure.spill(&picture);
                let file = format!("{base}_{class}_{n}");
                let path = dir.join(format!("{file}.png"));
                picture.save(&path).with_context(|| format!("writing {}", path.display()))?;
                if let buzz_scene::ObjectKind::Shape(shape) = &child.kind {
                    let b = kurbo::Shape::bounding_box(&shape.path);
                    eprintln!(
                        "shape {file}: spill_px={px} spill_beyond_1px={beyond} spill_max_px={max:.2} fill {:?} stroke {:?} bbox ({:.1},{:.1})-({:.1},{:.1})
  {}",
                        shape.fill.as_ref().map(|f| f.color().to_rgba8().to_u8_array()),
                        shape.stroke.as_ref().map(|s| s.width),
                        b.x0,
                        b.y0,
                        b.x1,
                        b.y1,
                        path_report(&shape.path)
                    );
                    let svg = dir.join(format!("{file}.svg.txt"));
                    std::fs::write(&svg, shape.path.to_svg()).with_context(|| format!("writing {}", svg.display()))?;
                } else {
                    eprintln!("shape {file}: spill_px={px} spill_beyond_1px={beyond} spill_max_px={max:.2} (not a shape object)");
                }
            }
        }
        if !found {
            eprintln!("--shapes {wanted}: no such rig part");
        }
    }

    println!(
        "{}",
        serde_json::json!({
            "name": name,
            "source_px": [w, h],
            "shapes": shapes,
            "pipeline_s": r2(pipeline_s),
            "mean_de": r2(mean_de),
            "edge_mean_px": r2(edge_mean),
            "edge_p95_px": r2(edge_p95),
            "small_features": format!("{matched}/{total}"),
            "small_features_pct": r2(pct),
            "ink_coverage_pct": r2(ink_coverage),
            "thin_ink_px": ink_total,
            "ink_shapes": ink_shapes.len(),
            "spill_px": spill_px,
            "spill_beyond_1px": spill_beyond_1px,
            "spill_max_px": r2(spill_max_px),
        })
    );
    Ok(())
}
