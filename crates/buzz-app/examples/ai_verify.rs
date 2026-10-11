//! C1-C4 quality run: runs the real character pipeline end-to-end (real
//! DWPose detection, real SAM2 part masks and LaMa completion when those
//! models are installed, real ColourLines tracing) on the six ground-truth
//! fixtures, measures each chunk's target against real per-part truth data,
//! and writes review PNGs.
//!
//! Usage:
//!   cargo run --release -p buzz-app --example ai_verify -- <out_dir>
//!
//! Fixtures (`crates/buzz-ai/tests/data/`): `pujariFront_0`, `sadhuSide_0`,
//! `ThakurBhaiStraight_copy_2_0` (bhatta2.fla); `OldmanfrontWalk_0`,
//! `sadhu1FrontWalk_0`, `sadhuSideWalk_0` (sustipur1.fla). Each has a
//! `<fixture>.joints.json` (true joint positions, from the rig itself via
//! `render_symbol --joints --crop1024`) and a `<fixture>.parts/` directory
//! of per-slot truth silhouette masks (`render_symbol --parts`), rendered in
//! the exact same cropped, 1024px-tall pixel space as the main PNG.
//!
//! Honesty note on left/right: a fixture's truth side names come from its
//! FLA's own "left"/"right" layer names, which are not always in COCO's
//! subject-relative convention DWPose outputs (some source files name
//! layers screen-relative, mirrored for a front-facing pose — verified
//! against `pujariFront`). Whichever global alignment (as-labelled, or every
//! " L"/" R" swapped) fits the detector's C1 joints better is used for that
//! fixture's C1 *and* C2 comparisons, so a labelling-convention mismatch
//! doesn't read as detector or segmentation error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};

use buzz_ai::character::{self, Skeleton};
use buzz_ai::models::ModelId;
use buzz_ai::onnx;
use buzz_ai::pose;
use buzz_ai::segment;
use buzz_geom::{BezPath, PathEl, Point};

const FIXTURES: &[&str] = &[
    "pujariFront_0",
    "sadhuSide_0",
    "ThakurBhaiStraight_copy_2_0",
    "OldmanfrontWalk_0",
    "sadhu1FrontWalk_0",
    "sadhuSideWalk_0",
];

/// C2 visible-IoU categories (item 5a of the "redesign C2-C4" review):
/// "torso, head, big limbs" at >= 0.75, "forearms, hands" at >= 0.6. "Big
/// limbs" (`upper_limbs`) is the upper arm and thigh bones.
const CATEGORIES: &[(&str, &[&str])] = &[
    ("torso", &["Chest", "Hips"]),
    ("head", &["Head"]),
    ("upper_limbs", &["Shoulder L", "Shoulder R", "Thigh L", "Thigh R"]),
    ("forearms", &["Elbow L", "Elbow R"]),
    ("hands", &["Hand L", "Hand R"]),
];

/// C1 joint categories: "arms and torso" vs "legs reported separately".
const C1_ARMS_TORSO_JOINTS: &[&str] = &[
    "Head", "Neck", "Chest", "Hips", "Shoulder L", "Elbow L", "Hand L", "Shoulder R", "Elbow R", "Hand R",
];
const C1_LEG_JOINTS: &[&str] = &["Thigh L", "Knee L", "Foot L", "Thigh R", "Knee R", "Foot R"];

fn draw_circle(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, color: [u8; 4]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && px < w && py >= 0 && py < h {
                    img.put_pixel(px as u32, py as u32, Rgba(color));
                }
            }
        }
    }
}

fn draw_line(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, thickness: i32, color: [u8; 4]) {
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        draw_circle(img, x, y, thickness, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}

fn load_true_joints(json_path: &Path) -> Result<Vec<(String, (f64, f64))>> {
    let content = std::fs::read_to_string(json_path)
        .with_context(|| format!("read {json_path:?}"))?;
    Ok(serde_json::from_str(&content)?)
}

/// Max (and mean) joint error, as a percentage of character height, over
/// the joints present in the ground truth.
fn joint_error_pct(detected: &Skeleton, true_joints: &[(String, (f64, f64))]) -> (f32, f32) {
    let height = detected.character_height.max(1.0);
    let mut max_err = 0.0f32;
    let mut sum_err = 0.0f32;
    let mut n = 0u32;
    for (name, (tx, ty)) in true_joints {
        if let Some(j) = detected.joint(name) {
            let dx = j.x as f64 - tx;
            let dy = j.y as f64 - ty;
            let dist = (dx * dx + dy * dy).sqrt() as f32;
            let pct = dist / height * 100.0;
            max_err = max_err.max(pct);
            sum_err += pct;
            n += 1;
        }
    }
    (max_err, if n > 0 { sum_err / n as f32 } else { 0.0 })
}

/// Same as [`joint_error_pct`], restricted to joint names in `subset`.
fn joint_error_pct_subset(detected: &Skeleton, true_joints: &[(String, (f64, f64))], subset: &[&str]) -> f32 {
    let filtered: Vec<(String, (f64, f64))> =
        true_joints.iter().filter(|(name, _)| subset.contains(&name.as_str())).cloned().collect();
    joint_error_pct(detected, &filtered).0
}

/// Swaps a single " L"/" R"-suffixed name's side.
fn swap_name(name: &str) -> String {
    if let Some(stem) = name.strip_suffix(" L") {
        format!("{stem} R")
    } else if let Some(stem) = name.strip_suffix(" R") {
        format!("{stem} L")
    } else {
        name.to_string()
    }
}

/// Swaps every " L"/" R" suffix in the ground-truth joint names. Used to
/// test whether a global left/right naming-convention mismatch (screen
/// side vs. character's anatomical side, a mismatch between this fixture's
/// authoring and DWPose's COCO-WholeBody convention) explains the error,
/// separately from real detector error.
fn swap_lr(true_joints: &[(String, (f64, f64))]) -> Vec<(String, (f64, f64))> {
    true_joints.iter().map(|(name, pt)| (swap_name(name), *pt)).collect()
}

fn mask_iou(a: &[u8], b: &[u8]) -> f32 {
    let mut inter: u64 = 0;
    let mut union: u64 = 0;
    for i in 0..a.len().min(b.len()) {
        let av = a[i] > 128;
        let bv = b[i] > 128;
        if av || bv {
            union += 1;
        }
        if av && bv {
            inter += 1;
        }
    }
    if union == 0 {
        1.0
    } else {
        inter as f32 / union as f32
    }
}

/// Counts interior alpha "holes" in a completed part: background pixels
/// (alpha <= 128) not reachable from the image border by 4-connectivity.
fn count_holes(rgba: &[u8], w: u32, h: u32) -> u32 {
    let w = w as i32;
    let h = h as i32;
    if w == 0 || h == 0 {
        return 0;
    }
    let alpha_at = |x: i32, y: i32| -> u8 { rgba[((y * w + x) * 4 + 3) as usize] };
    let mut reached = vec![false; (w * h) as usize];
    let mut stack: Vec<(i32, i32)> = Vec::new();
    let seed = |x: i32, y: i32, reached: &mut Vec<bool>, stack: &mut Vec<(i32, i32)>| {
        let idx = (y * w + x) as usize;
        if alpha_at(x, y) <= 128 && !reached[idx] {
            reached[idx] = true;
            stack.push((x, y));
        }
    };
    for x in 0..w {
        seed(x, 0, &mut reached, &mut stack);
        seed(x, h - 1, &mut reached, &mut stack);
    }
    for y in 0..h {
        seed(0, y, &mut reached, &mut stack);
        seed(w - 1, y, &mut reached, &mut stack);
    }
    while let Some((x, y)) = stack.pop() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx >= 0 && nx < w && ny >= 0 && ny < h {
                let nidx = (ny * w + nx) as usize;
                if !reached[nidx] && alpha_at(nx, ny) <= 128 {
                    reached[nidx] = true;
                    stack.push((nx, ny));
                }
            }
        }
    }
    let mut visited = vec![false; (w * h) as usize];
    let mut holes = 0u32;
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            if alpha_at(x, y) <= 128 && !reached[idx] && !visited[idx] {
                holes += 1;
                let mut st = vec![(x, y)];
                visited[idx] = true;
                while let Some((cx, cy)) = st.pop() {
                    for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let (nx, ny) = (cx + dx, cy + dy);
                        if nx >= 0 && nx < w && ny >= 0 && ny < h {
                            let nidx = (ny * w + nx) as usize;
                            if !visited[nidx] && !reached[nidx] && alpha_at(nx, ny) <= 128 {
                                visited[nidx] = true;
                                st.push((nx, ny));
                            }
                        }
                    }
                }
            }
        }
    }
    holes
}

/// Simulates rotating `part` by `angle_deg` around `joint` (its parent bone
/// head — the elbow for a forearm, the shoulder for an arm, etc.) and
/// measures the largest radius, within `radius` of the joint, at which the
/// rotated raster would show a gap (no opaque source pixel) rather than
/// remaining covered — i.e. how far the joint cap would visibly tear open.
/// 0 means no gap at any sampled radius.
fn rotation_gap_px(rgba: &[u8], part_w: u32, part_h: u32, joint: (f32, f32), radius: f32, angle_deg: f64) -> f32 {
    let (w, h) = (part_w as i32, part_h as i32);
    let alpha_at = |x: i32, y: i32| -> u8 {
        if x < 0 || y < 0 || x >= w || y >= h {
            0
        } else {
            rgba[((y * part_w as i32 + x) * 4 + 3) as usize]
        }
    };
    let (jx, jy) = (joint.0 as f64, joint.1 as f64);
    let theta = angle_deg.to_radians();
    let (cos_t, sin_t) = (theta.cos(), theta.sin());

    let mut max_gap = 0.0f32;
    let steps = 72;
    for ring in 1..=4 {
        let r = radius * ring as f32 / 4.0;
        for i in 0..steps {
            let a = (i as f64) / (steps as f64) * std::f64::consts::TAU;
            let (dx, dy) = (r as f64 * a.cos(), r as f64 * a.sin());
            // Where this post-rotation sample point came from before
            // rotating (inverse-rotate around the joint).
            let src_x = jx + dx * cos_t + dy * sin_t;
            let src_y = jy - dx * sin_t + dy * cos_t;
            if alpha_at(src_x.round() as i32, src_y.round() as i32) <= 128 {
                max_gap = max_gap.max(r);
            }
        }
    }
    max_gap
}

fn flatten_subpaths(path: &BezPath) -> Vec<Vec<Point>> {
    let mut subpaths: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    kurbo::flatten(path.iter(), 0.5, |el| match el {
        PathEl::MoveTo(p) => {
            if current.len() > 1 {
                subpaths.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
            current.push(p);
        }
        PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => current.push(p),
        PathEl::ClosePath => {
            if current.len() > 1 {
                subpaths.push(std::mem::take(&mut current));
            }
        }
    });
    if current.len() > 1 {
        subpaths.push(current);
    }
    subpaths
}

// --- Exact Euclidean distance transform (item 2 of the "Opus review of the
// redesign" next run: "Use an exact Euclidean distance transform (reuse
// `trace_lines`' `edt`) with no cap for `line_dist`."). `buzz_scene`'s
// `trace_lines::edt`/`dt_1d` (Felzenszwalb & Huttenlocher) is `pub(crate)`
// to that crate, so this is a byte-for-byte copy into the scorer, which is
// the only cross-crate way to reuse it without widening `buzz_scene`'s
// public API.
const EDT_INF: f64 = 1e18;

/// Exact distance from every pixel to the nearest zero (`false`) pixel,
/// treating outside the image as zero too.
fn edt(ink: &[bool], w: usize, h: usize) -> Vec<f64> {
    let pw = w + 2;
    let ph = h + 2;
    let mut g = vec![EDT_INF; pw * ph];
    for y in 0..h {
        for x in 0..w {
            let idx = (y + 1) * pw + (x + 1);
            g[idx] = if ink[y * w + x] { EDT_INF } else { 0.0 };
        }
    }
    for x in 0..pw {
        g[x] = 0.0;
        g[(ph - 1) * pw + x] = 0.0;
    }
    for y in 0..ph {
        g[y * pw] = 0.0;
        g[y * pw + pw - 1] = 0.0;
    }

    let mut tmp = vec![0.0; pw * ph];
    let mut col = vec![0.0; ph];
    for x in 0..pw {
        for y in 0..ph {
            col[y] = g[y * pw + x];
        }
        let d = dt_1d(&col);
        for y in 0..ph {
            tmp[y * pw + x] = d[y];
        }
    }
    let mut out = vec![0.0; pw * ph];
    let mut row = vec![0.0; pw];
    for y in 0..ph {
        for x in 0..pw {
            row[x] = tmp[y * pw + x];
        }
        let d = dt_1d(&row);
        for x in 0..pw {
            out[y * pw + x] = d[x];
        }
    }

    let mut r = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            r[y * w + x] = out[(y + 1) * pw + (x + 1)].sqrt();
        }
    }
    r
}

fn dt_1d(f: &[f64]) -> Vec<f64> {
    let n = f.len();
    let mut d = vec![0.0; n];
    if n == 0 {
        return d;
    }
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let mut s;
        loop {
            let p = v[k];
            s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q as f64 - p as f64));
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, slot) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        *slot = (q as f64 - p as f64).powi(2) + f[p];
    }
    d
}

/// Unbounded, exact distance (px) from every pixel to the nearest matte
/// boundary crossing (inside -> outside or outside -> inside), by combining
/// the two one-sided `edt` transforms. No search window, no cap.
fn boundary_distance_field(matte: &image::GrayImage) -> (Vec<f32>, u32, u32) {
    let (w, h) = (matte.width(), matte.height());
    let (wu, hu) = (w as usize, h as usize);
    let inside: Vec<bool> = matte.pixels().map(|p| p[0] > 128).collect();
    let outside: Vec<bool> = inside.iter().map(|&b| !b).collect();
    let dist_in = edt(&inside, wu, hu);
    let dist_out = edt(&outside, wu, hu);
    let field: Vec<f32> = (0..inside.len())
        .map(|i| if inside[i] { dist_in[i] as f32 } else { dist_out[i] as f32 })
        .collect();
    (field, w, h)
}

/// **Open item 1 (F7 round): the distance to the source's own lines**, not
/// just the silhouette. `boundary_distance_field` scores every traced stroke
/// against the matte outline only — but most traced strokes are interior
/// ink (eyes, mustache, collar, buttons, folds) that sits, correctly, far
/// from the outline, so its mean (14.6 px) measured where the lines are, not
/// how well they were traced. Here the source edges are the source's own ink
/// (dark pixels inside the matte: luma < 90) plus strong colour steps
/// (neighbour ΔRGB > 60) plus the matte boundary; the field is the exact
/// distance to the nearest of them.
fn source_edge_distance_field(img: &RgbaImage, matte: &image::GrayImage) -> (Vec<f32>, u32, u32) {
    let (w, h) = (matte.width(), matte.height());
    let (wu, hu) = (w as usize, h as usize);
    let inside = |x: u32, y: u32| matte.get_pixel(x, y)[0] > 128;
    let px = |x: u32, y: u32| {
        let p = img.get_pixel(x.min(img.width() - 1), y.min(img.height() - 1));
        [p[0] as i32, p[1] as i32, p[2] as i32]
    };
    let mut not_edge = vec![true; wu * hu];
    for y in 0..h {
        for x in 0..w {
            let here = inside(x, y);
            let c = px(x, y);
            let luma = (c[0] * 299 + c[1] * 587 + c[2] * 114) / 1000;
            let mut edge = here && luma < 90;
            for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                if nx < w && ny < h {
                    if inside(nx, ny) != here {
                        edge = true;
                    } else if here {
                        let d = px(nx, ny);
                        let step = (c[0] - d[0]).abs() + (c[1] - d[1]).abs() + (c[2] - d[2]).abs();
                        if step > 60 {
                            edge = true;
                        }
                    }
                }
            }
            if edge {
                not_edge[y as usize * wu + x as usize] = false;
            }
        }
    }
    let d = edt(&not_edge, wu, hu);
    (d.iter().map(|&v| v as f32).collect(), w, h)
}

/// Exact boundary distance (px) at `p`, clamped to the field's bounds. No
/// cap: a point far from any boundary crossing simply reads a large value.
fn line_dist_at(field: &[f32], w: u32, h: u32, p: Point) -> f32 {
    if w == 0 || h == 0 {
        return 0.0;
    }
    let px = (p.x.round() as i64).clamp(0, w as i64 - 1) as u32;
    let py = (p.y.round() as i64).clamp(0, h as i64 - 1) as u32;
    field[(py * w + px) as usize]
}

fn fill_polygon(img: &mut RgbaImage, subpaths: &[Vec<Point>], color: [u8; 4]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    if subpaths.is_empty() {
        return;
    }
    let min_y = subpaths
        .iter()
        .flatten()
        .map(|p| p.y)
        .fold(f64::MAX, f64::min)
        .floor()
        .max(0.0) as i32;
    let max_y = subpaths
        .iter()
        .flatten()
        .map(|p| p.y)
        .fold(f64::MIN, f64::max)
        .ceil()
        .min((h - 1) as f64) as i32;
    for y in min_y..=max_y {
        let yc = y as f64 + 0.5;
        let mut xs: Vec<f64> = Vec::new();
        for sp in subpaths {
            let n = sp.len();
            if n < 2 {
                continue;
            }
            for i in 0..n {
                let a = sp[i];
                let b = sp[(i + 1) % n];
                if (a.y <= yc && b.y > yc) || (b.y <= yc && a.y > yc) {
                    let t = (yc - a.y) / (b.y - a.y);
                    xs.push(a.x + t * (b.x - a.x));
                }
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut i = 0;
        while i + 1 < xs.len() {
            let x0 = xs[i].round() as i32;
            let x1 = xs[i + 1].round() as i32;
            for x in x0.max(0)..x1.min(w) {
                img.put_pixel(x as u32, y as u32, Rgba(color));
            }
            i += 2;
        }
    }
}

fn stroke_polyline(img: &mut RgbaImage, subpaths: &[Vec<Point>], width: f64, color: [u8; 4]) {
    let r = ((width / 2.0).round() as i32).max(1);
    for sp in subpaths {
        for i in 0..sp.len().saturating_sub(1) {
            draw_line(
                img,
                sp[i].x.round() as i32,
                sp[i].y.round() as i32,
                sp[i + 1].x.round() as i32,
                sp[i + 1].y.round() as i32,
                r,
                color,
            );
        }
    }
}

/// Renders one traced part's shapes alone onto a transparent `w x h`
/// buffer, using the same rasteriser as `traced.png` (item 5c: "C3 is holes
/// in the final part vectors").
fn render_traced_part(part: &character::TracedPart, w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for shape in part.caps.iter().chain(&part.shapes) {
        let subpaths = flatten_subpaths(&shape.path);
        if let Some(fill) = &shape.fill {
            let c = fill.color().to_rgba8();
            fill_polygon(&mut img, &subpaths, [c.r, c.g, c.b, c.a]);
        }
        if let Some(stroke) = &shape.stroke {
            let c = stroke.color().to_rgba8();
            stroke_polyline(&mut img, &subpaths, stroke.width, [c.r, c.g, c.b, c.a]);
        }
    }
    img
}

/// Loads the topmost-part label map `render_symbol --parts` writes
/// alongside the per-slot truth masks: a 1-based index image plus the slot
/// name each index means. `None` when the fixture hasn't been regenerated.
fn load_topmost(parts_dir: &Path) -> Option<(image::GrayImage, Vec<String>)> {
    let png = image::open(parts_dir.join("_topmost.png")).ok()?.to_luma8();
    let json = std::fs::read_to_string(parts_dir.join("_topmost.json")).ok()?;
    let names: Vec<String> = serde_json::from_str(&json).ok()?;
    Some((png, names))
}

/// Edge pixel coordinates (px, py) of a truth `_topmost.png` label index
/// image: every pixel whose value differs from its right or down neighbour.
/// Used by `overlay_labels.png` (item 1) to compare truth and predicted part
/// boundaries in the same pixel space.
fn gray_label_edges(label: &image::GrayImage) -> Vec<(i32, i32)> {
    let (w, h) = (label.width() as i32, label.height() as i32);
    let mut edges = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let v = label.get_pixel(x as u32, y as u32)[0];
            let right = if x + 1 < w { label.get_pixel((x + 1) as u32, y as u32)[0] } else { v };
            let down = if y + 1 < h { label.get_pixel(x as u32, (y + 1) as u32)[0] } else { v };
            if v != right || v != down {
                edges.push((x, y));
            }
        }
    }
    edges
}

/// Same as [`gray_label_edges`], for the predicted `color_palette_image()`
/// (compares full RGBA, so the transparent/no-part background also counts
/// as its own label and the character's outer silhouette shows as an edge).
fn rgba_label_edges(label: &RgbaImage) -> Vec<(i32, i32)> {
    let (w, h) = (label.width() as i32, label.height() as i32);
    let mut edges = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let v = label.get_pixel(x as u32, y as u32);
            let right = if x + 1 < w { *label.get_pixel((x + 1) as u32, y as u32) } else { *v };
            let down = if y + 1 < h { *label.get_pixel(x as u32, (y + 1) as u32) } else { *v };
            if *v != right || *v != down {
                edges.push((x, y));
            }
        }
    }
    edges
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// sRGB -> CIELAB (D65).
fn rgb_to_lab(r: u8, g: u8, b: u8) -> [f32; 3] {
    let (rl, gl, bl) = (srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
    let x = rl * 0.4124564 + gl * 0.3575761 + bl * 0.1804375;
    let y = rl * 0.2126729 + gl * 0.7151522 + bl * 0.0721750;
    let z = rl * 0.0193339 + gl * 0.1191920 + bl * 0.9503041;
    let (xn, yn, zn) = (0.95047f32, 1.0f32, 1.08883f32);
    let f = |t: f32| -> f32 { if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 } };
    let (fx, fy, fz) = (f(x / xn), f(y / yn), f(z / zn));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn delta_e76(c1: [u8; 3], c2: [u8; 3]) -> f32 {
    let l1 = rgb_to_lab(c1[0], c1[1], c1[2]);
    let l2 = rgb_to_lab(c2[0], c2[1], c2[2]);
    ((l1[0] - l2[0]).powi(2) + (l1[1] - l2[1]).powi(2) + (l1[2] - l2[2]).powi(2)).sqrt()
}

/// Mean ΔE (CIELAB) between `rendered` and `source` over pixels opaque in
/// both (item 5c/4: "ΔE <= 6, whole-character render vs source").
fn mean_delta_e(rendered: &RgbaImage, source: &RgbaImage) -> f32 {
    let mut sum = 0.0f64;
    let mut n = 0u64;
    let (w, h) = (rendered.width().min(source.width()), rendered.height().min(source.height()));
    for y in 0..h {
        for x in 0..w {
            let r = rendered.get_pixel(x, y);
            let s = source.get_pixel(x, y);
            if r[3] > 128 && s[3] > 128 {
                sum += delta_e76([r[0], r[1], r[2]], [s[0], s[1], s[2]]) as f64;
                n += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { (sum / n as f64) as f32 }
}

/// Hue-spread distinct tint colours for `parts_vectors.png`, one per part.
fn tint_for_index(i: usize, n: usize) -> [u8; 3] {
    let hue = (i as f32 / n.max(1) as f32) * 360.0;
    let (h, s, v) = (hue / 60.0, 0.65f32, 1.0f32);
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
}

/// Renders one traced part's shapes with its fill blended 50/50 toward a
/// distinct per-part tint colour (strokes keep their own ink colour so
/// linework stays legible), optionally rotated by `angle_deg` about `pivot`.
fn render_tinted_part(
    part: &character::TracedPart,
    tint: [u8; 3],
    pivot: Option<((f32, f32), f64)>,
    img: &mut RgbaImage,
) {
    let rotate = |p: Point| -> Point {
        let Some((joint, angle_deg)) = pivot else { return p };
        let theta = angle_deg.to_radians();
        let (jx, jy) = (joint.0 as f64, joint.1 as f64);
        let (dx, dy) = (p.x - jx, p.y - jy);
        Point::new(jx + dx * theta.cos() - dy * theta.sin(), jy + dx * theta.sin() + dy * theta.cos())
    };
    for shape in part.caps.iter().chain(&part.shapes) {
        let subpaths: Vec<Vec<Point>> =
            flatten_subpaths(&shape.path).into_iter().map(|sp| sp.into_iter().map(rotate).collect()).collect();
        if let Some(fill) = &shape.fill {
            let c = fill.color().to_rgba8();
            let blended = [
                ((c.r as u16 + tint[0] as u16) / 2) as u8,
                ((c.g as u16 + tint[1] as u16) / 2) as u8,
                ((c.b as u16 + tint[2] as u16) / 2) as u8,
                c.a,
            ];
            fill_polygon(img, &subpaths, blended);
        }
        if let Some(stroke) = &shape.stroke {
            let c = stroke.color().to_rgba8();
            stroke_polyline(img, &subpaths, stroke.width, [c.r, c.g, c.b, c.a]);
        }
    }
}

#[derive(Default)]
struct Aggregate {
    c1_arms_torso_max: f32,
    c1_legs_max: f32,
    c1_ms_max: u128,
    c2_iou_min: BTreeMap<String, f32>,
    /// Truth parts with no visible pixels (open item 3), reported apart.
    c2_occluded: Vec<String>,
    c2_ms_max: u128,
    c3_holes_total: u32,
    c3_gap_max: f32,
    c3_ms_max: u128,
    c4_uniform_width: bool,
    c4_line_dist_max: f32,
    /// Every traced stroke point's exact, uncapped boundary distance (item
    /// 2/3), pooled across all fixtures, for the mean/p95 the review asks
    /// for.
    c4_line_dist_samples: Vec<f32>,
    c4_de_max: f32,
    c4_ms_max: u128,
    leg_fallback_used: BTreeMap<String, bool>,
    fixtures_ok: Vec<String>,
    fixtures_failed: Vec<String>,
}

fn main() -> Result<()> {
    let out_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: ai_verify <out_dir>")?;
    std::fs::create_dir_all(&out_dir)?;

    let data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../buzz-ai/tests/data");

    // Item 2: load every model once, in the background, before any timed
    // stage below runs — so the C1-C4 timings measured per fixture are warm
    // (inference only), not cold loads.
    println!("=== warming session pool ===");
    let warm = onnx::warm(&[
        ModelId::AnimeSegmentation,
        ModelId::DwPoseDetector,
        ModelId::DwPoseEstimator,
        ModelId::Sam2Encoder,
        ModelId::Sam2Decoder,
        ModelId::LaMa,
    ]);
    for (id, dur, loaded) in &warm {
        println!("  {:<28} loaded={loaded} {} ms", id.name(), dur.as_millis());
    }
    let sam2_used = onnx::pooled(ModelId::Sam2Encoder).is_some() && onnx::pooled(ModelId::Sam2Decoder).is_some();
    let lama_used = onnx::pooled(ModelId::LaMa).is_some();
    println!("sam2_used={sam2_used} lama_used={lama_used}");

    let mut agg = Aggregate {
        c4_uniform_width: true,
        ..Default::default()
    };

    for &fixture in FIXTURES {
        println!("\n=== fixture: {fixture} ===");
        let png_path = data_dir.join(format!("{fixture}.png"));
        let joints_path = data_dir.join(format!("{fixture}.joints.json"));
        let parts_dir = data_dir.join(format!("{fixture}.parts"));

        let img = match image::open(&png_path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                println!("  FAILED to open {png_path:?}: {e}");
                agg.fixtures_failed.push(fixture.to_string());
                continue;
            }
        };
        println!("  size {}x{}", img.width(), img.height());
        let true_joints = load_true_joints(&joints_path)?;

        // --- C1: real matte + real DWPose detection + skeleton mapping ---
        let t0 = Instant::now();
        let matte = segment::matte(&img, None).context("matte")?;
        let persons = pose::detect(&img).context("pose::detect")?;
        if persons.is_empty() {
            println!("  FAILED: DWPose detected no person");
            agg.fixtures_failed.push(fixture.to_string());
            continue;
        }
        let mut skel = character::skeleton(&persons[0]);
        // Item 6: silhouette-split fallback when DWPose couldn't tell the
        // legs apart.
        let leg_fallback_fired = character::leg_fallback_silhouette_split(&mut skel, &matte);
        let c1_ms = t0.elapsed().as_millis();
        let (err_max_raw, err_mean_raw) = joint_error_pct(&skel, &true_joints);
        let swapped_truth = swap_lr(&true_joints);
        let (err_max_swapped, err_mean_swapped) = joint_error_pct(&skel, &swapped_truth);
        let (alignment, truth_aligned) = if err_mean_swapped < err_mean_raw {
            ("L/R-swapped", &swapped_truth)
        } else {
            ("as-labeled", &true_joints)
        };
        let arms_torso_err = joint_error_pct_subset(&skel, truth_aligned, C1_ARMS_TORSO_JOINTS);
        let legs_err = joint_error_pct_subset(&skel, truth_aligned, C1_LEG_JOINTS);
        println!(
            "  C1: [{alignment}] arms_torso={arms_torso_err:.2}% legs={legs_err:.2}% (as-labeled max={err_max_raw:.2}% mean={err_mean_raw:.2}%; L/R-swapped max={err_max_swapped:.2}% mean={err_mean_swapped:.2}%), leg_fallback={leg_fallback_fired}, {c1_ms} ms"
        );
        agg.c1_arms_torso_max = agg.c1_arms_torso_max.max(arms_torso_err);
        agg.c1_legs_max = agg.c1_legs_max.max(legs_err);
        agg.c1_ms_max = agg.c1_ms_max.max(c1_ms);
        agg.leg_fallback_used.insert(fixture.to_string(), leg_fallback_fired);

        // --- C2: part masks (real, SAM2-refined + item 2 cleaning when
        // installed), IoU against *visible* truth (item 5a): each truth
        // part mask intersected with the pixels where that part is the
        // topmost thing in the composite render, from the label map
        // `render_symbol --parts` writes alongside the truth masks.
        let t0 = Instant::now();
        let parts_real = character::part_masks(&img, &matte, &skel).context("part_masks")?;
        let c2_ms = t0.elapsed().as_millis();
        let topmost = load_topmost(&parts_dir);
        if topmost.is_none() {
            println!("    WARNING: no _topmost.png/_topmost.json for {fixture}; C2 IoU falls back to full (occlusion-unfair) truth");
        }
        for (cat, names) in CATEGORIES {
            let mut worst = 1.0f32;
            let mut found = false;
            for name in *names {
                // Truth files are named in the fixture's own (possibly
                // screen-relative) convention; apply the same alignment
                // decided by C1 so a labelling mismatch isn't scored as a
                // segmentation miss.
                let truth_name = if alignment == "L/R-swapped" { swap_name(name) } else { name.to_string() };
                let slug = truth_name.to_lowercase().replace(' ', "_");
                let truth_path = parts_dir.join(format!("{slug}.png"));
                let (Some(detected), Ok(truth_img)) = (parts_real.get(name), image::open(&truth_path)) else {
                    continue;
                };
                let truth_rgba = truth_img.to_rgba8();
                if truth_rgba.width() != parts_real.width || truth_rgba.height() != parts_real.height {
                    continue;
                }
                let mut truth_mask: Vec<u8> = truth_rgba.pixels().map(|p| p[3]).collect();
                if let Some((label_img, names_list)) = &topmost {
                    if let Some(pos) = names_list.iter().position(|n| n == &truth_name) {
                        let label_value = (pos + 1) as u8;
                        for (i, v) in truth_mask.iter_mut().enumerate() {
                            let (x, y) = (i as u32 % parts_real.width, i as u32 / parts_real.width);
                            if x >= label_img.width() || y >= label_img.height() || label_img.get_pixel(x, y)[0] != label_value {
                                *v = 0;
                            }
                        }
                    } else {
                        // This slot never won the topmost-pixel contest
                        // (fully occluded in the composite): visible truth
                        // is empty, not the full occluded truth.
                        truth_mask.iter_mut().for_each(|v| *v = 0);
                    }
                }
                // Open item 3: a part with no visible pixels at all (fully
                // occluded in the composite) has no visible truth to score
                // against — IoU is 0 by construction whatever the detector
                // did. Report it on its own line, outside the minimum.
                if truth_mask.iter().all(|&v| v < 128) {
                    println!("    occluded[{cat}] {truth_name}: no visible truth pixels — excluded from the IoU minimum");
                    agg.c2_occluded.push(format!("{fixture}:{truth_name}"));
                    continue;
                }
                let iou = mask_iou(&detected.mask, &truth_mask);
                worst = worst.min(iou);
                found = true;
            }
            if found {
                println!("    iou[{cat}] (real detected vs visible truth mask) = {worst:.3}");
                let e = agg.c2_iou_min.entry(cat.to_string()).or_insert(1.0);
                *e = e.min(worst);
            } else {
                println!("    iou[{cat}]: no truth mask available for this fixture");
            }
        }
        agg.c2_ms_max = agg.c2_ms_max.max(c2_ms);

        // --- C3: complete hidden parts. LaMa is off by default (item 4:
        // "High-quality fill", off by default) — `complete_parts` always
        // flat-fills, which is what keeps this stage inside the ≤10s
        // (without LaMa) warm target.
        let t0 = Instant::now();
        let completed =
            character::complete_parts(&img, &matte, &skel, &parts_real).context("complete_parts")?;
        let c3_ms = t0.elapsed().as_millis();

        // --- C4: item 1 (whole-character trace) + item 3 (split by part) +
        // item 4 (vector joint caps) in one pass ---
        let t0 = Instant::now();
        let traced = character::trace_character_whole(&completed, &skel, &matte, None)
            .context("trace_character_whole")?;
        let c4_ms = t0.elapsed().as_millis();

        // --- C3 quality (item 5c): holes in the *final part vectors*
        // (render each part alone, flood-fill test), plus the rotation-gap
        // proxy, both measured on the rendered vector output rather than
        // the raster completion. ---
        let mut fixture_holes = 0u32;
        let mut fixture_gap = 0.0f32;
        for part in &traced.parts {
            let rendered = render_traced_part(part, completed.width, completed.height);
            fixture_holes += count_holes(rendered.as_raw(), rendered.width(), rendered.height());
            if let Some(bone) = skel.bone(&part.name) {
                let radius = (bone.length * 0.2).max(6.0);
                for angle in [-60.0, 60.0] {
                    let gap =
                        rotation_gap_px(rendered.as_raw(), rendered.width(), rendered.height(), bone.head, radius, angle);
                    fixture_gap = fixture_gap.max(gap);
                }
            }
        }
        println!(
            "  C3: {fixture_holes} holes across {} final part vectors, rotation_gap(\u{b1}60\u{b0}) max={fixture_gap:.2}px, complete {c3_ms} ms",
            traced.parts.len()
        );
        agg.c3_holes_total += fixture_holes;
        agg.c3_gap_max = agg.c3_gap_max.max(fixture_gap);
        agg.c3_ms_max = agg.c3_ms_max.max(c3_ms);

        // --- C4 quality: uniform width, line_dist (item 5b: distance from
        // the whole-character traced strokes to the source edges), and ΔE
        // of the whole-character render vs source. ---
        let target_width = traced.uniform_width;
        let mut fixture_uniform = true;
        let mut fixture_line_dist = 0.0f32;
        let mut fixture_line_dist_samples: Vec<f32> = Vec::new();
        // Open item 1: scored against the source's own lines, see
        // `source_edge_distance_field`; the silhouette-only figure is kept
        // alongside for comparison with earlier runs.
        let (sil_field, _, _) = boundary_distance_field(&matte);
        let (dist_field, dist_w, dist_h) = source_edge_distance_field(&img, &matte);
        let mut sil_samples: Vec<f32> = Vec::new();
        let mut ordered_parts: Vec<_> = traced.parts.iter().collect();
        ordered_parts.sort_by_key(|p| p.depth);
        let mut traced_png = RgbaImage::new(completed.width, completed.height);
        for part in &ordered_parts {
            // The joint-cap exposed-arc outline (item 4) is synthetic
            // gap-prevention geometry, not a retrace of a source edge, so
            // item 5b's "distance to the source edges" excludes it. It lives
            // in `part.caps` now (behind every part), which this loop does not
            // visit, so nothing in `part.shapes` is excluded.
            let joint_cap_arc_idx: Option<usize> = None;
            for (i, shape) in part.shapes.iter().enumerate() {
                let subpaths = flatten_subpaths(&shape.path);
                if let Some(fill) = &shape.fill {
                    let c = fill.color().to_rgba8();
                    fill_polygon(&mut traced_png, &subpaths, [c.r, c.g, c.b, c.a]);
                }
                if let Some(stroke) = &shape.stroke {
                    let c = stroke.color().to_rgba8();
                    if (stroke.width - target_width).abs() > 1e-6 {
                        fixture_uniform = false;
                    }
                    if Some(i) != joint_cap_arc_idx {
                        for p in subpaths.iter().flatten() {
                            sil_samples.push(line_dist_at(&sil_field, dist_w, dist_h, *p));
                            let d = line_dist_at(&dist_field, dist_w, dist_h, *p);
                            if d > fixture_line_dist {
                                fixture_line_dist = d;
                            }
                            fixture_line_dist_samples.push(d);
                        }
                    }
                    stroke_polyline(&mut traced_png, &subpaths, stroke.width, [c.r, c.g, c.b, c.a]);
                }
            }
        }
        let fixture_de = mean_delta_e(&traced_png, &img);
        {
            let mean = |v: &[f32]| if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 };
            let p95 = |v: &[f32]| {
                let mut v = v.to_vec();
                v.sort_by(|a, b| a.total_cmp(b));
                v.get(((v.len() as f32) * 0.95) as usize).copied().unwrap_or(0.0)
            };
            println!(
                "  C4 line_dist to source lines: mean {:.2}px p95 {:.2}px; to the silhouette only (old scorer): mean {:.2}px p95 {:.2}px",
                mean(&fixture_line_dist_samples),
                p95(&fixture_line_dist_samples),
                mean(&sil_samples),
                p95(&sil_samples)
            );
        }
        println!(
            "  C4: uniform_width={fixture_uniform} (target {target_width:.2}px), max line_dist={fixture_line_dist:.2}px, mean \u{394}E={fixture_de:.2}, trace {c4_ms} ms"
        );
        agg.c4_uniform_width &= fixture_uniform;
        agg.c4_line_dist_max = agg.c4_line_dist_max.max(fixture_line_dist);
        agg.c4_line_dist_samples.extend_from_slice(&fixture_line_dist_samples);
        agg.c4_de_max = agg.c4_de_max.max(fixture_de);
        agg.c4_ms_max = agg.c4_ms_max.max(c4_ms);

        // --- Review PNGs ---
        let fdir = out_dir.join(fixture);
        std::fs::create_dir_all(&fdir)?;

        // overlay_joints.png (item 1): predicted joints (yellow/black) and
        // truth joints (magenta), in the *same pixel space* as the crop1024
        // fixture PNG so a coordinate mismatch is visible at a glance. Truth
        // is drawn using the same L/R alignment C1 scored against, so the
        // overlay shows exactly what was compared, not the raw file labels.
        let mut overlay_joints_png = img.clone();
        for bone in &skel.bones {
            if bone.confidence > 0.2 {
                draw_line(
                    &mut overlay_joints_png,
                    bone.head.0.round() as i32,
                    bone.head.1.round() as i32,
                    bone.tail.0.round() as i32,
                    bone.tail.1.round() as i32,
                    2,
                    [0, 230, 200, 255],
                );
            }
        }
        for joint in &skel.joints {
            if joint.confidence > 0.2 {
                draw_circle(&mut overlay_joints_png, joint.x.round() as i32, joint.y.round() as i32, 4, [20, 20, 20, 255]);
                draw_circle(&mut overlay_joints_png, joint.x.round() as i32, joint.y.round() as i32, 3, [255, 220, 0, 255]);
            }
        }
        for (_, (tx, ty)) in truth_aligned {
            draw_circle(&mut overlay_joints_png, tx.round() as i32, ty.round() as i32, 5, [255, 0, 220, 180]);
        }
        overlay_joints_png.save(fdir.join("overlay_joints.png"))?;

        // overlay_labels.png (item 1): the truth label-map edges (from
        // `_topmost.png`, red) over the predicted part label-map edges (from
        // `color_palette_image()`, cyan), both in crop1024 pixel space, so a
        // part-mask coordinate/scale mismatch is visible at a glance.
        let predicted_labels = parts_real.color_palette_image();
        let mut overlay_labels_png = img.clone();
        for (x, y) in rgba_label_edges(&predicted_labels) {
            draw_circle(&mut overlay_labels_png, x, y, 1, [0, 220, 255, 220]);
        }
        if let Some((truth_labels, _)) = &topmost {
            for (x, y) in gray_label_edges(truth_labels) {
                draw_circle(&mut overlay_labels_png, x, y, 1, [255, 30, 30, 220]);
            }
        }
        overlay_labels_png.save(fdir.join("overlay_labels.png"))?;

        predicted_labels.save(fdir.join("labels.png"))?;

        // parts_vectors.png: each part's own vectors in a distinct tint
        // (left half), plus the arms rotated 45\u{b0} about their shoulders to
        // show the joint caps (right half).
        let n = traced.parts.len();
        let mut normal_img = RgbaImage::new(completed.width, completed.height);
        let mut rotated_img = RgbaImage::new(completed.width, completed.height);
        for (i, part) in ordered_parts.iter().enumerate() {
            let tint = tint_for_index(i, n);
            render_tinted_part(part, tint, None, &mut normal_img);
            let is_arm = part.name.starts_with("Shoulder") || part.name.starts_with("Elbow") || part.name.starts_with("Hand");
            let pivot = if is_arm {
                let side = if part.name.ends_with('L') { "L" } else { "R" };
                skel.joint(&format!("Shoulder {side}")).map(|j| ((j.x, j.y), 45.0))
            } else {
                None
            };
            render_tinted_part(part, tint, pivot, &mut rotated_img);
        }
        let mut parts_vectors_png = RgbaImage::new(completed.width * 2, completed.height);
        image::imageops::overlay(&mut parts_vectors_png, &normal_img, 0, 0);
        image::imageops::overlay(&mut parts_vectors_png, &rotated_img, completed.width as i64, 0);
        parts_vectors_png.save(fdir.join("parts_vectors.png"))?;

        traced_png.save(fdir.join("traced.png"))?;
        println!("  wrote review PNGs -> {fdir:?}");

        agg.fixtures_ok.push(fixture.to_string());
    }

    println!("\n=== summary ===");
    println!("fixtures ok: {:?}", agg.fixtures_ok);
    println!("fixtures failed: {:?}", agg.fixtures_failed);
    println!("sam2_used={sam2_used} lama_used={lama_used}");
    println!("leg_fallback_used: {:?}", agg.leg_fallback_used);
    println!(
        "C1 max joint err %: arms_torso={:.2}% legs={:.2}%, ms(max)={}",
        agg.c1_arms_torso_max, agg.c1_legs_max, agg.c1_ms_max
    );
    println!("C2 visible iou (min across fixtures) by category: {:?}, ms(max)={}", agg.c2_iou_min, agg.c2_ms_max);
    println!("C2 fully occluded truth parts (excluded from the minimum): {:?}", agg.c2_occluded);
    println!(
        "C3 holes total (final part vectors): {}, rotation_gap_px(max)={:.2}, ms(max)={}",
        agg.c3_holes_total, agg.c3_gap_max, agg.c3_ms_max
    );
    let (line_dist_mean, line_dist_p95) = {
        let mut s = agg.c4_line_dist_samples.clone();
        if s.is_empty() {
            (0.0f32, 0.0f32)
        } else {
            let mean = s.iter().sum::<f32>() / s.len() as f32;
            s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p95_idx = (((s.len() as f64) * 0.95).ceil() as usize).saturating_sub(1).min(s.len() - 1);
            (mean, s[p95_idx])
        }
    };
    println!(
        "C4 uniform_width(all fixtures)={}, line_dist_px(max)={:.2} mean={:.2} p95={:.2} (n={}), \u{394}E(max)={:.2}, ms(max)={}",
        agg.c4_uniform_width,
        agg.c4_line_dist_max,
        line_dist_mean,
        line_dist_p95,
        agg.c4_line_dist_samples.len(),
        agg.c4_de_max,
        agg.c4_ms_max
    );
    let total_ms = agg.c1_ms_max + agg.c2_ms_max + agg.c3_ms_max + agg.c4_ms_max;
    println!("total_ms (sum of worst-case per-stage warm ms) = {total_ms}");

    Ok(())
}
