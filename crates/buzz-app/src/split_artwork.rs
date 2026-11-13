//! **Split traced artwork into layers** (SPLIT_PICTURE_PLAN S5).
//!
//! AI > Split Picture into Layers on vector shapes instead of a picture: the shapes
//! are rendered to a bitmap, the VLM + SAM 2 of `bg_objects` find the objects, and
//! every shape goes to the object whose mask covers most of it (at least 60 %); a filled
//! shape not taken whole is cut along the mask of **every** object whose reach overlaps it by
//! 150 px^2 or more, nearest object first, each piece to its object (S6, S13).
//! Each object becomes ONE library symbol holding its shapes unchanged, on ONE new
//! timeline layer `<Label> <n>` above the source layer. Shapes not taken stay where
//! they were; behind each object a filler (the mask region, hole-filled colour, traced)
//! is added to the source layer so hiding or moving the object leaves no hole. What no object takes
//! goes to the depth layers Sky, Far and Land: Sky and Land are SAM 2 masks prompted from the top
//! and bottom edge of the picture, and a filled shape straddling them is cut along their outlines
//! (S16); without SAM 2 the colour rule of `backdrop::split_plates` decides.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, Point, Rect, Shape};
use buzz_scene::{LayerId, LayerKind, Object, ObjectId, ObjectKind, Scene, ShapeData, SymbolKind};
use image::{GrayImage, RgbaImage};

use crate::bg_objects::ObjectOptions;

/// The smallest share of a shape an object's mask must cover to take it.
const TAKE_MIN: f64 = 0.6;
/// The same for a shape with an outline stroke.
const TAKE_MIN_STROKE: f64 = 0.25;
/// An object whose shapes paint less of its mask than this is dropped (it sits inside another).
const PAINTED_MIN: f64 = 0.5;
/// A filled shape not taken whole is cut along every object whose reach overlaps it by at least
/// this much (px^2 of the bitmap, S13).
const CUT_OVERLAP_MIN: f64 = 150.0;
/// Shapes with a smaller box than this (px^2 of the bitmap) are never cut.
const CUT_AREA_MIN: f64 = 64.0;
/// How far (px) an object's mask is grown when deciding which shapes belong to it.
const REACH: i32 = 4;
/// A stroke goes with an object only when this much of its box lies inside the object's box.
const STROKE_INSIDE_MIN: f64 = 0.6;
/// How far (px) the piece an object cuts off a shape runs past the cut, over what stays: the two
/// halves overlap instead of meeting edge to edge, where both soft edges let the background show
/// through as a pale line.
const SEAM_OVERLAP: i32 = 2;
/// The width of an object's outline (bitmap px) at the least: a bold line, whatever the artwork's.
const OUTLINE_WIDTH: f64 = 3.5;
/// How much of the pixel staircase a traced mask outline keeps (px; see `TraceOptions::detail`).
const OUTLINE_DETAIL: f64 = 1.5;
/// How much a traced mask outline's corners are rounded (`TraceOptions::smooth`).
const OUTLINE_SMOOTH: f64 = 0.6;
/// How much (px) a SAM 2 mask is smoothed before the shapes are cut along it.
const MASK_SMOOTH: f32 = 3.0;
/// How much (px) an object's silhouette is smoothed before its outline and its hole filler are traced.
const SILHOUETTE_SMOOTH: f32 = 1.5;
/// Holes in an object's silhouette smaller than this share of it are filled (the outline goes round them).
const SILHOUETTE_HOLE_MAX: f64 = 0.02;
/// Bits of an object's silhouette smaller than this (px) get no outline of their own.
const SILHOUETTE_ISLAND_MIN: usize = 150;
/// The fewest pixels round a hole its colour is read from.
const RING_MIN: usize = 20;
/// The longest side of the bitmap the VLM and SAM see.
const MAX_SIDE: f64 = 1920.0;
/// Depth layers by SAM 2 (S16): a plate whose mask covers more of the picture than this is rejected.
const DEPTH_MAX_COVER: f64 = 0.7;
/// A depth plate is also rejected when it covers more than this share of the rows (5 % of the
/// height) at the opposite edge: a sky that reaches the bottom, a ground that reaches the top.
const DEPTH_EDGE_MAX: f64 = 0.2;
/// Prompt points of the sky / ground masks: every `width / DEPTH_POINTS` along the edge.
const DEPTH_POINTS: u32 = 20;
/// The opposite edge gets a negative point every `width / DEPTH_NEG_POINTS`.
const DEPTH_NEG_POINTS: u32 = 8;
/// A prompt point sits on smooth picture: the largest step (CIELAB dE76) between 4-neighbours in
/// the 5 x 5 window round it is at most this.
const DEPTH_SMOOTH: f64 = 4.0;
/// SAM 2's mask (0..255, blurred) counts as set above this (127 is one half).
const DEPTH_THRESHOLD: u8 = 110;
/// Far is eroded by this many px before its parts are told apart, so a thin bridge between two
/// patches breaks (S16b).
const ISLAND_ERODE: i32 = 2;
/// A part of Far within this share of the longer side of the Sky mask touches the sky (S16b).
const ISLAND_SKY_REACH: f64 = 0.01;
/// The ground line of a column is the highest row from which down, over the next `GROUND_LINE_WINDOW`
/// of the height, Land holds this share of the pixels outside the objects (S16c). A window, not the
/// whole column below: the fog, bushes and hills above the ground stay Far.
const GROUND_LINE_SHARE: f64 = 0.6;
const GROUND_LINE_WINDOW: f64 = 0.05;
/// The ground lines are smoothed by a running median over this share of the width on each side.
const GROUND_LINE_SMOOTH: f64 = 0.01;
/// An object whose mask lies at least this much inside the Sky mask is sky, not an object (S16c).
const SKY_OBJECT_MIN: f64 = 0.7;
/// The sky is drawn once, by the fit (S18, S18b): a piece of Sky or Far is sky when, over its sample
/// points, the CIELAB dE76 to the fitted sky in their row has a mean of at most `SKY_MEAN_DE` and a
/// 95th percentile of at most `SKY_P95_DE`.
const SKY_MEAN_DE: f64 = 2.5;
const SKY_P95_DE: f64 = 5.0;
/// A texel of a texture on Sky or Far above the ground line, within `SKY_TEXEL_DE` dE76 of the fitted sky
/// in its picture row, is made see-through (S18b, S18c), and fades in up to `SKY_TEXEL_SOLID_DE` so a soft
/// glow keeps its falloff instead of ending in a ring.
const SKY_TEXEL_DE: f64 = 1.5;
const SKY_TEXEL_SOLID_DE: f64 = 4.5;
/// A piece of Sky that fails the test above moves to Far (a tree tip inside SAM's coarse mask) only
/// when its mean dE76 to the fit is above `SKY_TIP_DE` and a sample point lies this close to the sky
/// mask's outer edge (share of the longer side; S18c); else it stays (the moon's halo, clouds).
const SKY_TIP_DE: f64 = 8.0;
const SKY_EDGE_REACH: f64 = 0.01;
/// A sky-coloured piece of Far is removed only when it comes this close to the sky mask (share of the height).
const SKY_FAR_REACH: f64 = 0.03;
/// A whole shape goes to a depth layer holding at least this share of its sample points; below
/// that a filled shape is cut along the masks.
const PLATE_TAKE: f64 = 0.9;

/// A shape cut along an object's mask: the piece inside goes to the object, the rest stays.
/// Several objects may cut one shape (one `Cut` each, same `id`).
#[derive(Clone)]
pub struct Cut {
    pub id: ObjectId,
    /// The piece inside the mask, in the shape's own coordinates.
    pub inside: ShapeData,
    /// What stays on the source layer once **every** object took its piece (the same in all the
    /// cuts of one shape); `None` when nothing is left.
    pub rest: Option<ShapeData>,
}

/// The scene's depth layers, back to front (S9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plate {
    Sky,
    Far,
    Land,
}

/// One object and what goes with it.
pub struct ArtworkObject {
    /// `Some`: this is a depth layer (Sky / Far / Land), not an object; its `label` is the layer name.
    pub plate: Option<Plate>,
    /// For an object: the depth layer its hole filler goes into (behind that layer's shapes).
    pub behind: Plate,
    pub label: String,
    /// 1-based, per label.
    pub n: usize,
    /// `[x0, y0, x1, y1]` in rendered-bitmap pixels.
    pub bbox: [u32; 4],
    /// The SAM 2 mask (before the area its shapes cover is added).
    pub mask: GrayImage,
    /// The shapes it takes, in the source frame's paint order.
    pub shape_ids: Vec<ObjectId>,
    /// Shapes cut along the mask; their inside pieces go with the object too.
    pub cuts: Vec<Cut>,
    /// The hole filler, in pixels local to `filler_at`'s origin (a depth layer: its full-frame base).
    pub filler: Vec<ShapeData>,
    /// Filler pixels -> the source layer's space.
    pub filler_at: Affine,
    /// An object's outline round everything it took, in rendered-bitmap pixels (placed with the
    /// plan's `to_world`), drawn in the artwork's line style on top of its shapes so the cut
    /// edges read as drawn ones; `None` for a depth layer.
    pub outline: Option<ShapeData>,
}

/// What the worker thread decided; `apply` carries it out.
pub struct ArtworkPlan {
    pub layer: LayerId,
    pub frame: u32,
    pub objects: Vec<ArtworkObject>,
    pub notes: Vec<String>,
    /// The bitmap the shapes were rendered to.
    pub picture: RgbaImage,
    /// Bitmap pixels -> stage space.
    pub to_world: Affine,
    /// The run record folder (`split_cache`) this split's `result.json` goes into.
    pub record: Option<std::path::PathBuf>,
    /// Shapes the fitted sky draws instead (S18): `apply` deletes them from the source layer.
    pub removed: Vec<ObjectId>,
}

/// The shapes a split of `ids` on `layer` would use (pictures and instances are left alone).
pub fn source_shapes(scene: &Scene, layer: LayerId, frame: u32, ids: Option<&[ObjectId]>) -> Vec<Arc<Object>> {
    let Some(l) = scene.layers().get(layer) else { return Vec::new() };
    l.frames
        .objects_at(frame)
        .iter()
        .filter(|o| ids.is_none_or(|ids| ids.contains(&o.id)))
        .filter(|o| match &o.kind {
            // A picture is left alone; a traced region wearing the source's pixels (T16 Keep textures) is a shape.
            ObjectKind::Shape(sd) => match sd.fill.as_ref().map(|f| &f.paint) {
                Some(buzz_scene::Paint::Image(image)) => buzz_scene::is_trace_texture(&image.asset),
                _ => true,
            },
            _ => false,
        })
        .cloned()
        .collect()
}

/// Render only `shapes` (of `layer` at `frame`) to a bitmap of at most `MAX_SIDE` px.
/// Returns it with the bitmap-pixel -> stage-space affine.
pub fn render_shapes(scene: &Scene, layer: LayerId, frame: u32, shapes: &[Arc<Object>]) -> Result<(RgbaImage, Affine), String> {
    let (region, w, h) = view_of(scene, shapes)?;
    let img = render_in(scene, layer, frame, shapes, region, (w, h), false)?;
    Ok((img, Affine::translate((region.x0, region.y0)) * Affine::scale(region.width() / w as f64)))
}

/// The stage rectangle round `shapes` and the bitmap size it renders at.
fn view_of(scene: &Scene, shapes: &[Arc<Object>]) -> Result<(Rect, u32, u32), String> {
    let mut region: Option<Rect> = None;
    for o in shapes {
        let b = scene.resolved_bounds(o);
        if b.width().is_finite() && b.height().is_finite() {
            region = Some(region.map_or(b, |r| r.union(b)));
        }
    }
    let region = region.filter(|r| r.width() > 0.0 && r.height() > 0.0).ok_or("the artwork has no extent")?;
    let scale = (MAX_SIDE / region.width().max(region.height())).min(1.0);
    let (w, h) = ((region.width() * scale).ceil().max(1.0) as u32, (region.height() * scale).ceil().max(1.0) as u32);
    let region = Rect::new(region.x0, region.y0, region.x0 + w as f64 / scale, region.y0 + h as f64 / scale);
    Ok((region, w, h))
}

/// `shapes` alone on `layer` at `frame`, `region` of the stage at `size` px.
fn render_in(scene: &Scene, layer: LayerId, frame: u32, shapes: &[Arc<Object>], region: Rect, size: (u32, u32), transparent: bool) -> Result<RgbaImage, String> {
    let mut ex = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).map_err(|e| format!("no renderer: {e}"))?;
    render_on(&mut ex, scene, layer, frame, shapes, region, size, transparent)
}

/// As [`render_in`] on an exporter already up (starting one costs about two seconds, so a run of
/// renders shares one).
#[allow(clippy::too_many_arguments)]
fn render_on(ex: &mut buzz_export::Exporter, scene: &Scene, layer: LayerId, frame: u32, shapes: &[Arc<Object>], region: Rect, size: (u32, u32), transparent: bool) -> Result<RgbaImage, String> {
    let mut only = scene.clone();
    // The shapes are cut in their layer's own space, so the render must be of that space: a
    // scene camera (pan, zoom) would move the picture against the cuts made from it.
    only.camera_mut().enabled = false;
    // The timeline being edited: a symbol's own when one is open (what the exporter draws then),
    // the stage's otherwise. Only `layer` and the folders holding it stay visible.
    let mut keep = vec![layer];
    while let Some(parent) = keep.last().and_then(|id| only.layers().get(*id)).and_then(|l| l.parent) {
        keep.push(parent);
    }
    let ids: Vec<_> = only.layers().iter().map(|l| l.id).collect();
    for id in ids {
        only.update_layer(id, |l| {
            if id == layer {
                l.frames.set_objects(frame, shapes.to_vec());
                l.visible = true;
            } else if keep.contains(&id) {
                l.visible = true;
            } else {
                l.visible = false;
            }
        });
    }
    // Each render stands alone: nothing drawn for the last scene is reused for this one.
    ex.reset_caches();
    let mut settings = buzz_export::ExportSettings::for_stage(&only);
    settings.width = size.0;
    settings.height = size.1;
    settings.transparent = transparent;
    settings.region = Some(region);
    let f = ex.render(&only, frame, &settings).map_err(|e| format!("render failed: {e}"))?;
    RgbaImage::from_raw(f.width, f.height, f.pixels).ok_or_else(|| "render returned a bad bitmap".to_string())
}

/// Points standing for the shape in bitmap pixels: a grid inside a filled one, steps
/// along the line of a stroke-only one.
fn sample_points(sd: &ShapeData, to_px: Affine) -> Vec<Point> {
    let mut path: BezPath = sd.path.clone();
    path.apply_affine(to_px);
    let bb = path.bounding_box();
    let mut pts = Vec::new();
    if sd.fill.is_some() {
        let n = (((bb.width() * bb.height()) / 4.0).sqrt().ceil() as usize).clamp(1, 24);
        for j in 0..n {
            for i in 0..n {
                let p = Point::new(bb.x0 + bb.width() * (i as f64 + 0.5) / n as f64, bb.y0 + bb.height() * (j as f64 + 0.5) / n as f64);
                if path.winding(p) != 0 {
                    pts.push(p);
                }
            }
        }
        if pts.is_empty() {
            pts.push(bb.center());
        }
    } else {
        let mut last: Option<Point> = None;
        kurbo::flatten(path.iter(), 0.5, |el| match el {
            kurbo::PathEl::MoveTo(p) => {
                pts.push(p);
                last = Some(p);
            }
            kurbo::PathEl::LineTo(p) => {
                if let Some(a) = last {
                    let steps = ((a.distance(p) / 2.0).ceil() as usize).max(1);
                    for k in 1..=steps {
                        pts.push(a.lerp(p, k as f64 / steps as f64));
                    }
                }
                last = Some(p);
            }
            _ => {}
        });
        if pts.is_empty() {
            pts.push(bb.center());
        }
    }
    pts
}

/// Share of `pts` that fall on `mask`.
fn covered(mask: &GrayImage, pts: &[Point]) -> f64 {
    let (w, h) = mask.dimensions();
    let hit = pts
        .iter()
        .filter(|p| p.x >= 0.0 && p.y >= 0.0 && (p.x as u32) < w && (p.y as u32) < h && mask.get_pixel(p.x as u32, p.y as u32)[0] > 127)
        .count();
    hit as f64 / pts.len().max(1) as f64
}

/// How much of the filled shape `path` (bitmap pixels) lies on `mask` (px^2), estimated on a grid of
/// samples (3 px apart, coarser for a big overlap box so a pair costs at most ~2500 tests);
/// counting stops once `stop_at` is reached. `mask_box` is a box round the mask's set pixels.
fn overlap_area(path: &BezPath, mask: &GrayImage, mask_box: Rect, stop_at: f64) -> f64 {
    let clip = path.bounding_box().intersect(mask_box);
    if clip.area() <= 0.0 {
        return 0.0;
    }
    let (w, h) = mask.dimensions();
    let step = (clip.area() / 2500.0).sqrt().max(3.0);
    let cell = step * step;
    let mut area = 0.0;
    let mut y = clip.y0 + step * 0.5;
    while y < clip.y1 {
        let mut x = clip.x0 + step * 0.5;
        while x < clip.x1 {
            if x >= 0.0 && y >= 0.0 && (x as u32) < w && (y as u32) < h && mask.get_pixel(x as u32, y as u32)[0] > 127 && path.winding(Point::new(x, y)) != 0 {
                area += cell;
                if area >= stop_at {
                    return area;
                }
            }
            x += step;
        }
        y += step;
    }
    area
}

/// The box round the set pixels of `mask`, grown by `grow` px (empty when none is set).
fn mask_box(mask: &GrayImage, grow: f64) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (x, y, p) in mask.enumerate_pixels() {
        if p[0] > 127 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return Rect::ZERO;
    }
    Rect::new(x0 as f64 - grow, y0 as f64 - grow, x1 as f64 + grow, y1 as f64 + grow)
}

// -- depth layers by SAM 2 (S16) -----------------------------------------------------------------

/// The sky and ground masks SAM 2 gave (0 / 255); `None`: not found or rejected (the notes say why).
struct SamDepth {
    sky: Option<GrayImage>,
    /// Without the sky.
    land: Option<GrayImage>,
}

/// Is the picture smooth round `(x, y)`: no step above `DEPTH_SMOOTH` (CIELAB dE76) between a pixel
/// of the 5 x 5 window and its right or lower neighbour?
fn smooth_at(picture: &RgbaImage, x: u32, y: u32) -> bool {
    let (w, h) = picture.dimensions();
    let lab = |x: u32, y: u32| {
        let p = picture.get_pixel(x, y);
        buzz_scene::trace::srgb_to_lab([p[0], p[1], p[2]])
    };
    for yy in y.saturating_sub(2)..=(y + 2).min(h - 1) {
        for xx in x.saturating_sub(2)..=(x + 2).min(w - 1) {
            let a = lab(xx, yy);
            for (nx, ny) in [(xx + 1, yy), (xx, yy + 1)] {
                if nx < w && ny < h {
                    let b = lab(nx, ny);
                    if ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() > DEPTH_SMOOTH {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// `n` points along row `y` of a picture `w` wide, the first half a step in.
fn row_points(w: u32, y: u32, n: u32) -> Vec<(u32, u32)> {
    (0..n).map(|k| (w / (2 * n) + k * (w / n), y)).filter(|p| p.0 < w).collect()
}

/// Box blur of the `w` x `h` bytes `v` by `r` px (a window at the border averages what is inside).
fn box_blur(v: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut across = vec![0f32; w * h];
    let mut prefix = vec![0f32; w + 1];
    for y in 0..h {
        for x in 0..w {
            prefix[x + 1] = prefix[x] + v[y * w + x] as f32;
        }
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            across[y * w + x] = (prefix[b] - prefix[a]) / (b - a) as f32;
        }
    }
    let mut out = vec![0u8; w * h];
    let mut sum = vec![0f64; w];
    let (mut lo, mut hi) = (0usize, 0usize);
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
        while hi < b {
            for x in 0..w {
                sum[x] += across[hi * w + x] as f64;
            }
            hi += 1;
        }
        while lo < a {
            for x in 0..w {
                sum[x] -= across[lo * w + x] as f64;
            }
            lo += 1;
        }
        let n = (b - a) as f64;
        for x in 0..w {
            out[y * w + x] = (sum[x] / n).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// One SAM 2 mask for the prompt points, cleaned: blurred and thresholded, closed, only the parts
/// touching a positive point (within 2 px) kept, holes under 2 % of the picture filled (a moon,
/// stars). `Ok(None)`: nothing is left.
fn depth_mask(
    emb: &buzz_ai::segment::Sam2Embeddings,
    size: (u32, u32),
    positive: &[buzz_ai::segment::Sam2Point],
    negative: &[buzz_ai::segment::Sam2Point],
) -> Result<Option<Vec<u8>>, String> {
    let (w, h) = size;
    let points: Vec<_> = positive.iter().chain(negative).copied().collect();
    let mask = match buzz_ai::segment::sam2_mask(emb, &points, None) {
        Ok(Some((m, _))) if m.dimensions() == size => m,
        Ok(Some(_)) => return Err("mask has the wrong size".into()),
        Ok(None) => return Err("the SAM 2 decoder is not installed".into()),
        Err(e) => return Err(format!("SAM 2: {e}")),
    };
    // The decoder's mask is a 256 x 256 grid (about 8 px a cell at 1920 wide) whose uncertain parts
    // checker: blur it by about a cell before the threshold and close the result by a cell, so a
    // part is not cut into cells (measured on the user's picture: 5 % of it was found as sky).
    let side = w.max(h);
    let blurred = box_blur(mask.as_raw(), w as usize, h as usize, (side / 240).max(2) as usize);
    let bin: Vec<u8> = blurred.iter().map(|&v| if v > DEPTH_THRESHOLD { 255 } else { 0 }).collect();
    let mut closed = buzz_ai::segment::morph_close(&bin, w, h, (side / 320).max(3) as i32);
    // A closing never loses a pixel; undo the erosion the edge of the picture gives.
    for (c, b) in closed.iter_mut().zip(&bin) {
        if *b > 0 {
            *c = 255;
        }
    }
    let parts = buzz_ai::segment::connected_components(&closed, w, h);
    let mut part_at = vec![u32::MAX; closed.len()];
    for (k, part) in parts.iter().enumerate() {
        for &i in part {
            part_at[i] = k as u32;
        }
    }
    let mut keep = vec![false; parts.len()];
    for &(px, py, _) in positive {
        for y in (py as i32 - 2).max(0)..=(py as i32 + 2).min(h as i32 - 1) {
            for x in (px as i32 - 2).max(0)..=(px as i32 + 2).min(w as i32 - 1) {
                let k = part_at[(y as u32 * w + x as u32) as usize];
                if k != u32::MAX {
                    keep[k as usize] = true;
                }
            }
        }
    }
    let mut kept = vec![0u8; closed.len()];
    for (part, _) in parts.iter().zip(&keep).filter(|(_, k)| **k) {
        for &i in part {
            kept[i] = 255;
        }
    }
    if !kept.iter().any(|&v| v > 0) {
        return Ok(None);
    }
    let fg = kept.iter().filter(|&&v| v > 0).count().max(1) as f64;
    Ok(Some(buzz_ai::segment::fill_small_holes(&kept, w, h, (0.02 * w as f64 * h as f64 / fg) as f32)))
}

/// Why a depth mask is not taken: it covers too much of the picture, or of the rows at the
/// opposite edge (`top_rows`: the first 5 % of the rows, else the last 5 %). Else its share.
fn depth_verdict(mask: &[u8], size: (u32, u32), top_rows: bool) -> Result<f64, String> {
    let (w, h) = size;
    let share = mask.iter().filter(|&&v| v > 0).count() as f64 / (w as f64 * h as f64);
    if share > DEPTH_MAX_COVER {
        return Err(format!("it covers {:.0}% of the picture", share * 100.0));
    }
    let rows = ((h as f64 * 0.05).ceil() as u32).clamp(1, h);
    let (a, b) = if top_rows { (0, rows) } else { (h - rows, h) };
    let edge = mask[(a * w) as usize..(b * w) as usize].iter().filter(|&&v| v > 0).count() as f64 / ((b - a) * w) as f64;
    if edge > DEPTH_EDGE_MAX {
        return Err(format!("it covers {:.0}% of the {} rows", edge * 100.0, if top_rows { "top" } else { "bottom" }));
    }
    Ok(share)
}

/// **Sky and Land by SAM 2** (S16). Sky: positive points on the smooth top edge, negative points on
/// the bottom edge. Land: the same from the bottom edge (negative: the sky's points), without the
/// sky. (Negative points at the objects' centres, as first planned, turned SAM's masks into
/// lattices: sky 8 % instead of 14 % on the user's picture; the objects take their own shapes
/// first anyway.) Each is cleaned and rejected when it covers too much of the picture or of the
/// opposite edge; the reason goes to `notes`. `None` when SAM 2 could not be asked at all.
fn sam_depth(emb: &buzz_ai::segment::Sam2Embeddings, picture: &RgbaImage, notes: &mut Vec<String>) -> Option<SamDepth> {
    let size = picture.dimensions();
    let (w, h) = size;
    let inset = (h / 100).max(3);
    if h < 8 * inset {
        return None;
    }
    let smooth_row = |y: u32| -> Vec<buzz_ai::segment::Sam2Point> {
        row_points(w, y, DEPTH_POINTS).into_iter().filter(|&(x, y)| smooth_at(picture, x, y)).map(|(x, y)| (x as f32, y as f32, true)).collect()
    };
    let row_negatives = |y: u32| -> Vec<buzz_ai::segment::Sam2Point> { row_points(w, y, DEPTH_NEG_POINTS).into_iter().map(|(x, y)| (x as f32, y as f32, false)).collect() };
    let top = smooth_row(inset);
    let sky = if top.is_empty() {
        notes.push("depth layers: no smooth picture along the top edge, so no sky".into());
        None
    } else {
        match depth_mask(emb, size, &top, &row_negatives(h - inset)) {
            Ok(Some(m)) => match depth_verdict(&m, size, false) {
                Ok(_) => Some(m),
                Err(why) => {
                    notes.push(format!("depth layers: no sky ({why})"));
                    None
                }
            },
            Ok(None) => {
                notes.push("depth layers: SAM 2 found no sky".into());
                None
            }
            Err(e) => {
                notes.push(format!("depth layers: {e}; colour rule used"));
                return None;
            }
        }
    };
    let bottom = smooth_row(h - inset);
    let land = if bottom.is_empty() {
        notes.push("depth layers: no smooth picture along the bottom edge; ground by the colour rule".into());
        None
    } else {
        let negative: Vec<buzz_ai::segment::Sam2Point> = top.iter().map(|&(x, y, _)| (x, y, false)).collect();
        match depth_mask(emb, size, &bottom, &negative) {
            Ok(Some(mut m)) => {
                if let Some(sky) = &sky {
                    for (l, s) in m.iter_mut().zip(sky) {
                        if *s > 0 {
                            *l = 0;
                        }
                    }
                }
                match depth_verdict(&m, size, true) {
                    Ok(_) => Some(m),
                    Err(why) => {
                        notes.push(format!("depth layers: ground by the colour rule ({why})"));
                        None
                    }
                }
            }
            Ok(None) => {
                notes.push("depth layers: SAM 2 found no ground; ground by the colour rule".into());
                None
            }
            Err(e) => {
                notes.push(format!("depth layers: {e}; colour rule used"));
                return None;
            }
        }
    };
    Some(SamDepth { sky: sky.and_then(|m| GrayImage::from_raw(w, h, m)), land: land.and_then(|m| GrayImage::from_raw(w, h, m)) })
}

/// **Ground line per column** (S16c): in each column the ground line `g(x)` is the highest row `y`
/// from which down (rows `y..y + GROUND_LINE_WINDOW * h`) `land` holds `GROUND_LINE_SHARE` of the
/// pixels outside `objects` (none: no line). The lines are smoothed by a running median over `GROUND_LINE_SMOOTH` of the
/// width on each side (a column without a line counts as below the picture), and every Far pixel
/// (in neither mask) at or below its column's line joins `land`; pixels of the objects are left
/// alone. Returns the share of the picture moved, in percent, and the smoothed line of each column
/// (a row; the height when there is none).
fn ground_line(sky: &GrayImage, objects: &[&GrayImage], land: &mut GrayImage) -> (f64, Vec<usize>) {
    let (w, h) = sky.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let mut in_object = vec![false; wu * hu];
    for m in objects {
        for (o, v) in in_object.iter_mut().zip(m.iter()) {
            *o |= *v > 127;
        }
    }
    let window = ((h as f64 * GROUND_LINE_WINDOW).round() as usize).max(1);
    let mut line = vec![hu; wu];
    // Per column: the Land and the non-object pixels from each row down (index `hu`: none).
    let (mut ground_from, mut all_from) = (vec![0u32; hu + 1], vec![0u32; hu + 1]);
    for (x, g) in line.iter_mut().enumerate() {
        for y in (0..hu).rev() {
            let i = y * wu + x;
            let free = !in_object[i];
            all_from[y] = all_from[y + 1] + free as u32;
            ground_from[y] = ground_from[y + 1] + (free && land.as_raw()[i] > 127) as u32;
        }
        for y in (0..hu).rev() {
            let end = (y + window).min(hu);
            let (ground, all) = (ground_from[y] - ground_from[end], all_from[y] - all_from[end]);
            if all > 0 && ground as f64 >= GROUND_LINE_SHARE * all as f64 {
                *g = y;
            }
        }
    }
    let reach = ((w as f64 * GROUND_LINE_SMOOTH).round() as usize).max(1);
    let smooth: Vec<usize> = (0..wu)
        .map(|x| {
            let mut near = line[x.saturating_sub(reach)..(x + reach + 1).min(wu)].to_vec();
            near.sort_unstable();
            near[near.len() / 2]
        })
        .collect();
    let ground: &mut [u8] = &mut *land;
    let mut moved = 0usize;
    for (x, &g) in smooth.iter().enumerate() {
        for y in g..hu {
            let i = y * wu + x;
            if !in_object[i] && sky.as_raw()[i] <= 127 && ground[i] <= 127 {
                ground[i] = 255;
                moved += 1;
            }
        }
    }
    (moved as f64 * 100.0 / (wu * hu) as f64, smooth)
}

/// **Ground stays together** (S16b): every connected part of Far (the pixels in neither mask) that
/// does not come within `ISLAND_SKY_REACH` of the sky is ground SAM 2's mask left out, so it joins
/// `land`. Far is eroded by `ISLAND_ERODE` first so a thin bridge breaks; the pixels the erosion
/// loses follow the nearest part (breadth first inside Far), and Far too thin to leave a part is a
/// part of its own. No sky mask: nothing to be away from, nothing moves. Returns the share of the
/// picture moved, in percent.
fn ground_islands(sky: &GrayImage, land: &mut GrayImage) -> f64 {
    let (w, h) = sky.dimensions();
    let n = (w * h) as usize;
    if !sky.iter().any(|&v| v > 127) {
        return 0.0;
    }
    let far: Vec<u8> = sky.iter().zip(land.iter()).map(|(s, l)| if *s > 127 || *l > 127 { 0 } else { 255 }).collect();
    let mut part = vec![u32::MAX; n];
    let mut parts = 0u32;
    for comp in buzz_ai::segment::connected_components(&buzz_ai::segment::erode(&far, w, h, ISLAND_ERODE), w, h) {
        for i in comp {
            part[i] = parts;
        }
        parts += 1;
    }
    let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|&i| part[i] != u32::MAX).collect();
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w as usize, i / w as usize);
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let j = ny as usize * w as usize + nx as usize;
            if far[j] > 0 && part[j] == u32::MAX {
                part[j] = part[i];
                queue.push_back(j);
            }
        }
    }
    let rest: Vec<u8> = (0..n).map(|i| if far[i] > 0 && part[i] == u32::MAX { 255 } else { 0 }).collect();
    for comp in buzz_ai::segment::connected_components(&rest, w, h) {
        for i in comp {
            part[i] = parts;
        }
        parts += 1;
    }
    let not_sky = GrayImage::from_raw(w, h, sky.iter().map(|&v| if v > 127 { 0 } else { 255 }).collect()).unwrap_or_else(|| GrayImage::new(w, h));
    let dist = buzz_ai::segment::distance_transform(&not_sky);
    let reach = (w.max(h) as f64 * ISLAND_SKY_REACH) as f32;
    let mut touches = vec![false; parts as usize];
    for i in 0..n {
        if part[i] != u32::MAX && dist[i] <= reach {
            touches[part[i] as usize] = true;
        }
    }
    let mut moved = 0usize;
    for (i, l) in land.iter_mut().enumerate() {
        if part[i] != u32::MAX && !touches[part[i] as usize] {
            *l = 255;
            moved += 1;
        }
    }
    moved as f64 * 100.0 / n as f64
}

/// The depth layer behind an object: Land when the pixels just under its feet are Land (or it
/// stands on the bottom edge of the picture), else Far.
fn behind_of(land: &GrayImage, mask: &GrayImage) -> Plate {
    let (w, h) = mask.dimensions();
    let Some(bottom) = (0..h).rev().find(|&y| (0..w).any(|x| mask.get_pixel(x, y)[0] > 127)) else {
        return Plate::Land;
    };
    if bottom + 1 >= h {
        return Plate::Land;
    }
    let (mut on, mut all) = (0u32, 0u32);
    for x in 0..w {
        if (bottom.saturating_sub(2)..=bottom).any(|y| mask.get_pixel(x, y)[0] > 127) {
            for y in bottom + 1..(bottom + 7).min(h) {
                all += 1;
                on += (land.get_pixel(x, y)[0] > 127) as u32;
            }
        }
    }
    if on * 2 >= all { Plate::Land } else { Plate::Far }
}

/// The Sky layer's base: the vertical ramp and glows `fit_sky` fits to the sky's own pixels, the
/// ramp stretched over the whole frame (it pads past its ends) so hiding anything above it never
/// shows a hole. Pixel space.
fn sky_base_of(picture: &RgbaImage, sky: &GrayImage) -> Vec<ShapeData> {
    let mut plate = picture.clone();
    for (p, m) in plate.pixels_mut().zip(sky.iter()) {
        p[3] = if *m > 127 { 255 } else { 0 };
    }
    let mut shapes = crate::backdrop::fit_sky(&plate).shapes;
    if let Some(ramp) = shapes.first_mut() {
        ramp.path = Rect::new(0.0, 0.0, picture.width() as f64, picture.height() as f64).to_path(0.1);
    }
    shapes
}

/// The fitted sky's CIELAB colour in each of the `h` rows: the ramp `sky_base_of` made (the first
/// shape of the fit), read at the middle of the row (it pads past its ends). Empty without a ramp.
fn sky_rows(fit: &[ShapeData], h: u32) -> Vec<[f64; 3]> {
    let Some(buzz_scene::Paint::Gradient(ramp)) = fit.first().and_then(|s| s.fill.as_ref()).map(|f| &f.paint) else { return Vec::new() };
    let det = ramp.transform.determinant();
    if det.abs() < 1e-12 || !det.is_finite() {
        return Vec::new();
    }
    // Unit x (-1..1) runs down the ramp's rectangle.
    let back = ramp.transform.inverse();
    (0..h)
        .map(|y| {
            let c = ramp.sample((back * Point::new(0.0, y as f64 + 0.5)).x * 0.5 + 0.5).to_rgba8().to_u8_array();
            buzz_scene::trace::srgb_to_lab([c[0], c[1], c[2]])
        })
        .collect()
}

/// The colour `sd` paints at each of `pts` (bitmap pixels; `m` takes its coordinates to them): a
/// solid or gradient fill its (average) colour, a texture its pixel there; `None` where nothing is
/// painted.
fn paint_at(sd: &ShapeData, m: Affine, pts: &[Point]) -> Vec<Option<[u8; 3]>> {
    let Some(fill) = sd.fill.as_ref() else { return vec![None; pts.len()] };
    match &fill.paint {
        buzz_scene::Paint::Image(image) => {
            let det = m.determinant();
            if det.abs() < 1e-12 || !det.is_finite() {
                return vec![None; pts.len()];
            }
            let back = m.inverse();
            pts.iter()
                .map(|p| {
                    let px = image.to_pixel(back * *p)?;
                    let c = image.asset.pixel(px.x.floor() as i64, px.y.floor() as i64);
                    (c[3] > 127).then_some([c[0], c[1], c[2]])
                })
                .collect()
        }
        paint => {
            let c = paint.color().to_rgba8().to_u8_array();
            vec![Some([c[0], c[1], c[2]]); pts.len()]
        }
    }
}

/// What `sky_by_fit` decides for one piece of Sky.
enum SkyVerdict {
    Keep,
    Remove,
    ToFar,
}

/// The CIELAB dE76 between two colours.
fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// A copy of the texture `image` (`m` takes its unit square to bitmap pixels) where every texel
/// above the ground line `ground` that lies within `SKY_TEXEL_DE` of the fitted sky `rows` in its
/// picture row is see-through and fades in up to `SKY_TEXEL_SOLID_DE` (S18b), with the number of
/// texels made (partly) see-through; `None` when there is none.
/// The copy wears the placeholder id of a trace's texture: `apply` puts it in the library.
fn sky_texture(image: &buzz_scene::ImageFill, m: Affine, rows: &[[f64; 3]], ground: &[usize], size: (u32, u32)) -> Option<(Arc<buzz_scene::ImageAsset>, usize)> {
    let (iw, ih) = (image.asset.width as usize, image.asset.height as usize);
    if iw == 0 || ih == 0 || image.asset.pixels.len() != iw * ih * 4 {
        return None;
    }
    let mut pixels = image.asset.pixels.as_ref().clone();
    let mut cleared = 0usize;
    for j in 0..ih {
        for i in 0..iw {
            let at = (j * iw + i) * 4;
            if pixels[at + 3] == 0 {
                continue;
            }
            let p = m * Point::new((i as f64 + 0.5) / iw as f64, (j as f64 + 0.5) / ih as f64);
            if !(p.x >= 0.0 && p.y >= 0.0 && p.x < size.0 as f64 && p.y < size.1 as f64) {
                continue;
            }
            let (x, y) = (p.x as usize, p.y as usize);
            if y >= ground.get(x).copied().unwrap_or(size.1 as usize) {
                continue;
            }
            let d = delta_e(buzz_scene::trace::srgb_to_lab([pixels[at], pixels[at + 1], pixels[at + 2]]), rows[y]);
            if d < SKY_TEXEL_SOLID_DE {
                let keep = ((d - SKY_TEXEL_DE) / (SKY_TEXEL_SOLID_DE - SKY_TEXEL_DE)).clamp(0.0, 1.0);
                pixels[at + 3] = (pixels[at + 3] as f64 * keep).round() as u8;
                cleared += 1;
            }
        }
    }
    (cleared > 0).then(|| {
        let copy = buzz_scene::ImageAsset::from_pixels(buzz_scene::trace::TEXTURE_PLACEHOLDER, "Trace texture", iw as u32, ih as u32, Arc::new(pixels));
        (Arc::new(copy), cleared)
    })
}

/// What `sky_by_fit` did.
#[derive(Default)]
struct SkyFitted {
    /// Shapes to delete from the source (all their pieces removed).
    removed: Vec<ObjectId>,
    /// Pieces removed from Sky, moved from Sky to Far, removed from Far.
    counts: [usize; 3],
    /// Textures on Sky or Far copied with the sky opened, their texels cleared, and all their texels.
    opened: [usize; 3],
}

/// **The sky is drawn once, by the fit** (S18). `fit` is the Sky layer's base (`sky_base_of`) and
/// `sky` the sky mask; every filled, stroke-less piece the plates took (`ids` the whole shapes,
/// `cuts` the cut pieces, per plate) is read against the fitted ramp in its rows by the colours at
/// its sample points. On Sky: a mean dE `SKY_MEAN_DE` and a 95th percentile `SKY_P95_DE` at most, the
/// fit draws it, so it is removed; else, with a mean dE above `SKY_TIP_DE` and a point within
/// `SKY_EDGE_REACH` of the mask's outer edge (the mask with its enclosed holes filled, so the hole
/// round a moon is no edge), it goes to Far (a tree tip); else it stays (the moon and its halo,
/// stars, clouds; S18c). On Far: the same test, every point above the ground line `ground` and one
/// within `SKY_FAR_REACH` of the sky mask, and it is removed (a gap in the foliage shows the Sky
/// layer). Strokes are left alone. Last, a piece on Sky or Far wearing a texture gets a copy of it
/// with the sky's texels see-through (`sky_texture`, S18b, S18c; a whole shape becomes a cut piece,
/// as the plates' own pieces are, so the copy is its own).
#[allow(clippy::too_many_arguments)]
fn sky_by_fit(
    shapes: &[Arc<Object>],
    remain: &[Option<BezPath>],
    sky: &GrayImage,
    ground: &[usize],
    fit: &[ShapeData],
    to_px: Affine,
    ids: &mut [Vec<ObjectId>; 3],
    cuts: &mut [Vec<Cut>; 3],
) -> SkyFitted {
    let (w, h) = sky.dimensions();
    let rows = sky_rows(fit, h);
    if rows.is_empty() {
        return SkyFitted::default();
    }
    // Distance to the sky mask (`d_out`: outside it, to the nearest sky pixel). The outer edge is the
    // edge of the mask with its enclosed holes filled (a moon or star hole is no edge, S18c): inside it
    // `e_in` runs to the nearest pixel that is not sky, outside it `e_out` to the nearest sky pixel.
    let outside = |m: &[u8]| GrayImage::from_raw(w, h, m.iter().map(|&v| if v > 127 { 0 } else { 255 }).collect()).unwrap_or_else(|| GrayImage::new(w, h));
    let d_out = buzz_ai::segment::distance_transform(&outside(sky.as_raw()));
    let filled = GrayImage::from_raw(w, h, buzz_ai::segment::fill_small_holes(sky.as_raw(), w, h, f32::MAX)).unwrap_or_else(|| sky.clone());
    let (e_in, e_out) = (buzz_ai::segment::distance_transform(&filled), buzz_ai::segment::distance_transform(&outside(filled.as_raw())));
    let (edge_reach, far_reach) = ((w.max(h) as f64 * SKY_EDGE_REACH) as f32, (h as f64 * SKY_FAR_REACH) as f32);
    let (col, row) = (|p: &Point| (p.x.max(0.0) as usize).min(w as usize - 1), |p: &Point| (p.y.max(0.0) as usize).min(h as usize - 1));
    let pos: std::collections::HashMap<ObjectId, usize> = shapes.iter().enumerate().map(|(k, s)| (s.id, k)).collect();
    // Whether `sd` (shape `k`) is sky: over its painted sample points the dE to the fitted sky in
    // their row has a mean of at most `SKY_MEAN_DE` and a 95th percentile of at most `SKY_P95_DE`;
    // that mean (infinite with no painted point) and the points. `None` for a stroke or an unfilled shape.
    let read = |k: usize, sd: &ShapeData| -> Option<(bool, f64, Vec<Point>)> {
        if sd.stroke.is_some() || sd.fill.is_none() {
            return None;
        }
        let m = to_px * shapes[k].transform;
        let pts = sample_points(sd, m);
        let mut de: Vec<f64> = paint_at(sd, m, &pts).iter().zip(&pts).filter_map(|(c, p)| c.map(|c| delta_e(buzz_scene::trace::srgb_to_lab(c), rows[row(p)]))).collect();
        de.sort_by(f64::total_cmp);
        let sky = !de.is_empty() && de.iter().sum::<f64>() <= SKY_MEAN_DE * de.len() as f64 && de[((de.len() as f64 * 0.95).ceil() as usize).clamp(1, de.len()) - 1] <= SKY_P95_DE;
        let mean = if de.is_empty() { f64::INFINITY } else { de.iter().sum::<f64>() / de.len() as f64 };
        Some((sky, mean, pts))
    };
    let on_sky = |k: usize, sd: &ShapeData| match read(k, sd) {
        None => SkyVerdict::Keep,
        Some((true, _, _)) => SkyVerdict::Remove,
        Some((false, mean, pts)) => {
            let edge = |p: &Point| {
                let i = row(p) * w as usize + col(p);
                if filled.as_raw()[i] > 127 { e_in[i] } else { e_out[i] }
            };
            if mean > SKY_TIP_DE && pts.iter().any(|p| edge(p) <= edge_reach) { SkyVerdict::ToFar } else { SkyVerdict::Keep }
        }
    };
    let on_far = |k: usize, sd: &ShapeData| {
        read(k, sd).is_some_and(|(sky, _, pts)| {
            sky && pts.iter().all(|p| row(p) < ground.get(col(p)).copied().unwrap_or(h as usize))
                && pts.iter().any(|p| d_out[row(p) * w as usize + col(p)] <= far_reach)
        })
    };
    // The shape as it is now: what the objects left of it, else whole.
    let current = |k: usize| match &shapes[k].kind {
        ObjectKind::Shape(sd) => Some(ShapeData { path: remain[k].clone().unwrap_or_else(|| sd.path.clone()), ..sd.clone() }),
        _ => None,
    };
    let (mut removed, mut counts) = (Vec::new(), [0usize; 3]);
    // Cut shapes with a piece removed: deleted from the source when no piece of them is left.
    let mut cut_gone: Vec<ObjectId> = Vec::new();
    // Far first (only what it held), then Sky, whose tree tips join Far.
    ids[1].retain(|id| {
        let gone = pos.get(id).and_then(|&k| current(k).map(|sd| on_far(k, &sd))).unwrap_or(false);
        if gone {
            removed.push(*id);
            counts[2] += 1;
        }
        !gone
    });
    for c in std::mem::take(&mut cuts[1]) {
        if pos.get(&c.id).is_some_and(|&k| on_far(k, &c.inside)) {
            cut_gone.push(c.id);
            counts[2] += 1;
        } else {
            cuts[1].push(c);
        }
    }
    let mut to_far: Vec<ObjectId> = Vec::new();
    ids[0].retain(|id| {
        let verdict = pos.get(id).and_then(|&k| current(k).map(|sd| on_sky(k, &sd))).unwrap_or(SkyVerdict::Keep);
        match verdict {
            SkyVerdict::Keep => true,
            SkyVerdict::Remove => {
                removed.push(*id);
                counts[0] += 1;
                false
            }
            SkyVerdict::ToFar => {
                to_far.push(*id);
                counts[1] += 1;
                false
            }
        }
    });
    ids[1].extend(to_far);
    for c in std::mem::take(&mut cuts[0]) {
        match pos.get(&c.id).map_or(SkyVerdict::Keep, |&k| on_sky(k, &c.inside)) {
            SkyVerdict::Keep => cuts[0].push(c),
            SkyVerdict::Remove => {
                cut_gone.push(c.id);
                counts[0] += 1;
            }
            SkyVerdict::ToFar => {
                counts[1] += 1;
                cuts[1].push(c);
            }
        }
    }
    for id in cut_gone {
        if !removed.contains(&id) && !cuts.iter().flatten().any(|c| c.id == id) {
            removed.push(id);
        }
    }
    // Textures on Sky and Far (S18b, S18c): the sky's texels see-through in a copy; the pieces wearing
    // the same texture in the same place share one copy.
    let mut opened = [0usize; 3];
    let mut copies: Vec<(*const buzz_scene::ImageAsset, [u64; 6], Option<Arc<buzz_scene::ImageAsset>>)> = Vec::new();
    let mut open = |k: usize, sd: &ShapeData| -> Option<ShapeData> {
        let Some(buzz_scene::Paint::Image(image)) = sd.fill.as_ref().map(|f| &f.paint) else { return None };
        if image.tile {
            return None;
        }
        let m = to_px * shapes[k].transform * image.transform;
        let key = (Arc::as_ptr(&image.asset), m.as_coeffs().map(f64::to_bits));
        let at = match copies.iter().position(|c| c.0 == key.0 && c.1 == key.1) {
            Some(at) => at,
            None => {
                let copy = sky_texture(image, m, &rows, ground, (w, h)).map(|(copy, cleared)| {
                    opened[0] += 1;
                    opened[1] += cleared;
                    opened[2] += copy.width as usize * copy.height as usize;
                    copy
                });
                copies.push((key.0, key.1, copy));
                copies.len() - 1
            }
        };
        let copy = copies[at].2.clone()?;
        let mut sd = sd.clone();
        if let Some(buzz_scene::Paint::Image(image)) = sd.fill.as_mut().map(|f| &mut f.paint) {
            image.asset = copy;
        }
        Some(sd)
    };
    for plate in [0, 1] {
        for c in &mut cuts[plate] {
            if let Some(inside) = pos.get(&c.id).and_then(|&k| open(k, &c.inside)) {
                c.inside = inside;
            }
        }
        for id in std::mem::take(&mut ids[plate]) {
            match pos.get(&id).and_then(|&k| current(k).and_then(|sd| open(k, &sd))) {
                Some(inside) => cuts[plate].push(Cut { id, inside, rest: None }),
                None => ids[plate].push(id),
            }
        }
    }
    SkyFitted { removed, counts, opened }
}

/// What `assign_to_plates` decided for one shape.
enum Fate {
    /// The whole shape to this plate (0 Sky, 1 Far, 2 Land).
    Whole(usize),
    /// Cut along the Land then the Sky mask; the plate that takes it whole if the cut fails.
    Cut(usize),
}

/// `cur` (a shape's own coordinates; `m` takes them to bitmap pixels) cut along each outline in
/// turn (plate, outline in pixels, worth cutting): the pieces inside as `[Sky, Far, Land]`, what is
/// left in Far. `None` when the shape's transform cannot be undone.
fn cut_along(cur: &BezPath, m: Affine, outlines: [(usize, &BezPath, bool); 2]) -> Option<[BezPath; 3]> {
    let det = m.determinant();
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let back = m.inverse();
    let mut pieces = [BezPath::new(), BezPath::new(), BezPath::new()];
    let mut rest = cur.clone();
    for (plate, outline, wanted) in outlines {
        if !wanted || outline.elements().is_empty() || rest.elements().is_empty() {
            continue;
        }
        let mut local = outline.clone();
        local.apply_affine(back);
        let bo = buzz_geom::boolean::options_for(&[rest.clone(), local.clone()]).fast();
        let inside = buzz_geom::boolean::boolean(&rest, &local, buzz_geom::boolean::BoolOp::Intersect, bo);
        if inside.elements().is_empty() {
            continue;
        }
        let after = buzz_geom::boolean::boolean(&rest, &local, buzz_geom::boolean::BoolOp::Difference, bo);
        pieces[plate] = inside;
        rest = after;
    }
    pieces[1] = rest;
    Some(pieces)
}

/// **Shapes to depth layers by the plate masks** (S16) for every shape no object kept
/// (`remain`: what the objects left of the shapes they cut). A shape with a stroke goes whole to
/// the nearest plate holding 25 % of it (Land, Far, Sky). A filled one goes whole to a plate
/// holding `PLATE_TAKE` of it, else it is cut along the Land mask outline and then the Sky mask
/// outline (same boolean code as the object cuts; the pieces abut): pieces to Land and Sky, the
/// rest to Far. Returns, per plate (Sky, Far, Land), the whole shapes in paint order and the
/// cut pieces. A whole shape that paints after one on a nearer plate that it really overlaps (a
/// filled one holds a sample point of the other, not just a box meeting a box: with the box test
/// rows of ink strokes far from the road ended on Land) moves up to that plate; the pieces keep
/// the paint order inside their layer and never move.
fn assign_to_plates(
    shapes: &[Arc<Object>],
    kept: &std::collections::HashSet<ObjectId>,
    remain: &[Option<BezPath>],
    masks: &[(Plate, GrayImage); 3],
    to_px: Affine,
    opts: &ObjectOptions,
) -> ([Vec<ObjectId>; 3], [Vec<Cut>; 3]) {
    use rayon::prelude::*;
    struct Left {
        k: usize,
        /// The path now, in the shape's own coordinates.
        cur: BezPath,
        /// Share of its sample points on each plate.
        share: [f64; 3],
        fate: Fate,
        /// At most 16 of its sample points (bitmap pixels), for the paint-order check.
        probe: Vec<Point>,
    }
    /// A whole shape for the paint-order check: the plate it goes to, its path and box in bitmap
    /// pixels, whether it is filled, and its sample points.
    struct Whole {
        id: ObjectId,
        plate: usize,
        bbox: Rect,
        path: BezPath,
        filled: bool,
        probe: Vec<Point>,
    }
    // The shapes really overlap: a filled one holds a sample point of the other.
    let overlap = |a: &Whole, b: &Whole| {
        a.bbox.intersect(b.bbox).area() > 0.0
            && ((a.filled && b.probe.iter().any(|p| a.path.winding(*p) != 0)) || (b.filled && a.probe.iter().any(|p| b.path.winding(*p) != 0)))
    };
    // Nearest plate first, so a tie goes to the nearer.
    const NEAR_FIRST: [usize; 3] = [2, 1, 0];
    let mut left: Vec<Left> = Vec::new();
    for (k, o) in shapes.iter().enumerate() {
        let ObjectKind::Shape(sd) = &o.kind else { continue };
        if kept.contains(&o.id) {
            continue;
        }
        let cur = remain[k].clone().unwrap_or_else(|| sd.path.clone());
        if cur.elements().is_empty() {
            continue;
        }
        let m = to_px * o.transform;
        let pts = sample_points(&ShapeData { path: cur.clone(), ..sd.clone() }, m);
        let share = [0, 1, 2].map(|i| covered(&masks[i].1, &pts));
        let best = NEAR_FIRST.into_iter().fold(2usize, |b, i| if share[i] > share[b] { i } else { b });
        let fate = if sd.stroke.is_some() || sd.fill.is_none() {
            Fate::Whole(NEAR_FIRST.into_iter().find(|&i| share[i] >= TAKE_MIN_STROKE).unwrap_or(best))
        } else if share[best] >= PLATE_TAKE {
            Fate::Whole(best)
        } else {
            let mut b = cur.clone();
            b.apply_affine(m);
            let bb = b.bounding_box();
            if bb.width() * bb.height() >= CUT_AREA_MIN { Fate::Cut(best) } else { Fate::Whole(best) }
        };
        let probe: Vec<Point> = pts.iter().step_by((pts.len() / 16).max(1)).take(16).copied().collect();
        left.push(Left { k, cur, share, fate, probe });
    }
    let cutting = |i: usize| left.iter().any(|l| matches!(l.fate, Fate::Cut(_)) && l.share[i] > 0.0);
    let (land_px, sky_px) = rayon::join(
        || if cutting(2) { mask_outline(&masks[2].1, opts) } else { BezPath::new() },
        || if cutting(0) { mask_outline(&masks[0].1, opts) } else { BezPath::new() },
    );
    let pieces: Vec<Option<[BezPath; 3]>> = left
        .par_iter()
        .map(|l| match l.fate {
            Fate::Cut(_) => cut_along(&l.cur, to_px * shapes[l.k].transform, [(2, &land_px, l.share[2] > 0.0), (0, &sky_px, l.share[0] > 0.0)]),
            Fate::Whole(_) => None,
        })
        .collect();
    let mut ids: [Vec<ObjectId>; 3] = Default::default();
    let mut cuts: [Vec<Cut>; 3] = Default::default();
    // The whole shapes, in paint order.
    let mut whole: Vec<Whole> = Vec::new();
    for (l, made) in left.iter().zip(pieces) {
        let o = &shapes[l.k];
        let ObjectKind::Shape(sd) = &o.kind else { continue };
        let whole_to = |plate: usize| {
            let mut path = l.cur.clone();
            path.apply_affine(to_px * o.transform);
            Whole { id: o.id, plate, bbox: path.bounding_box(), path, filled: sd.fill.is_some(), probe: l.probe.clone() }
        };
        match (&l.fate, made) {
            (Fate::Whole(plate), _) => whole.push(whole_to(*plate)),
            (Fate::Cut(plate), None) => whole.push(whole_to(*plate)),
            (Fate::Cut(plate), Some(pieces)) => {
                let live: Vec<usize> = (0..3).filter(|&i| !pieces[i].elements().is_empty()).collect();
                match live[..] {
                    [only] => whole.push(whole_to(only)),
                    [] => whole.push(whole_to(*plate)),
                    _ => {
                        for i in live {
                            cuts[i].push(Cut { id: o.id, inside: ShapeData { path: pieces[i].clone(), ..sd.clone() }, rest: None });
                        }
                    }
                }
            }
        }
    }
    for i in 0..whole.len() {
        let mut k = whole[i].plate;
        for j in 0..i {
            if whole[j].plate > k && overlap(&whole[j], &whole[i]) {
                k = whole[j].plate;
            }
        }
        whole[i].plate = k;
        ids[k].push(whole[i].id);
    }
    (ids, cuts)
}

/// Work out the split (worker thread): render, detect, assign shapes, trace the fillers.
pub fn plan_split(
    scene: &Scene,
    layer: LayerId,
    frame: u32,
    ids: Option<&[ObjectId]>,
    opts: &ObjectOptions,
    note: &mut dyn FnMut(&str),
) -> Result<ArtworkPlan, String> {
    let shapes = source_shapes(scene, layer, frame, ids);
    if shapes.is_empty() {
        return Err("Split Picture: there are no vector shapes to split".into());
    }
    note("Rendering the artwork");
    let (region, w, h) = view_of(scene, &shapes)?;
    let picture = render_in(scene, layer, frame, &shapes, region, (w, h), false)?;
    let to_world = Affine::translate((region.x0, region.y0)) * Affine::scale(region.width() / w as f64);
    let to_px = to_world.inverse();
    note("Finding the objects (VLM + SAM 2)");
    let mut notes = Vec::new();
    // An indoor scene (a room, a shop inside) has no sky: its furniture is asked for, and
    // everything that is not an object stays on one Room layer.
    let indoor = crate::bg_objects::scene_is_indoor(&picture, opts, &mut notes);
    let opts = &ObjectOptions { indoor, ..opts.clone() };
    // The SAM 2 embedding of the picture is made once: the objects and the sky / ground use it.
    let (mut found, emb) = crate::bg_objects::detect_objects_with(&picture, opts, &mut notes, true);
    let record = crate::split_cache::take_last_run();
    if found.is_empty() {
        notes.push("no objects found; nothing split".into());
    }
    // The shapes are cut along smooth curves, not along SAM 2's coarse, ragged edge. A thin thing
    // (a pole, a pen) that the smoothing would wear away is smoothed less, or kept as it is.
    for d in &mut found {
        let area = |m: &GrayImage| m.iter().filter(|&&v| v > 127).count();
        let before = area(&d.mask);
        if let Some(smooth) = [MASK_SMOOTH, 1.0].into_iter().map(|sigma| smooth_mask(&d.mask, sigma)).find(|m| area(m) as f64 >= 0.7 * before as f64) {
            d.mask = smooth;
        }
    }

    note("Splitting the depth layers");
    // Sky and Land by SAM 2 (S16); the colour rule of `split_plates` is the fallback for all of it
    // (no SAM 2) or for the ground (its mask rejected).
    let sam = emb.as_ref().filter(|_| !indoor).and_then(|e| sam_depth(e, &picture, &mut notes));
    let by_sam = sam.is_some();
    // An object whose mask lies mostly inside the sky is sky (a VLM box over the empty sky above a
    // roof, S16c): dropped, its shapes go to the plates as usual.
    if let Some(sky) = sam.as_ref().and_then(|s| s.sky.as_ref()) {
        found.retain(|d| {
            let (inside, all) = d.mask.iter().zip(sky.iter()).filter(|(m, _)| **m > 127).fold((0u64, 0u64), |(i, a), (_, s)| (i + (*s > 127) as u64, a + 1));
            let is_sky = inside as f64 >= SKY_OBJECT_MIN * all.max(1) as f64;
            if is_sky {
                notes.push(format!("objects: {} {:?} dropped (it is sky)", d.label, d.bbox));
            }
            !is_sky
        });
    }
    let plates = (!indoor && sam.as_ref().is_none_or(|s| s.land.is_none())).then(|| crate::backdrop::split_plates(&picture));
    let plate_mask = |a: &RgbaImage, b: Option<&RgbaImage>| {
        let mut m = GrayImage::new(w, h);
        for (x, y, p) in m.enumerate_pixels_mut() {
            let on = a.get_pixel(x, y)[3] > 128 || b.is_some_and(|b| b.get_pixel(x, y)[3] > 128);
            p[0] = if on { 255 } else { 0 };
        }
        m
    };
    let colour = plates.as_ref().map(|p| [plate_mask(&p.sky, None), plate_mask(&p.far, None), plate_mask(&p.mid, Some(&p.ground))]);
    // The ground line of each column (a row), for the sky pieces of Far (S18).
    let mut ground: Vec<usize> = Vec::new();
    let plate_masks: [(Plate, GrayImage); 3] = match (sam, colour) {
        // Indoors: no sky and no far plate; the whole room is the one plate behind the objects.
        _ if indoor => [(Plate::Sky, GrayImage::new(w, h)), (Plate::Far, GrayImage::new(w, h)), (Plate::Land, GrayImage::from_pixel(w, h, image::Luma([255])))],
        (Some(s), colour) => {
            let sky = s.sky.unwrap_or_else(|| GrayImage::new(w, h));
            let mut land = match (s.land, colour) {
                (Some(land), _) => land,
                (None, Some([_, _, mut land])) => {
                    for (l, s) in land.iter_mut().zip(sky.iter()) {
                        if *s > 127 {
                            *l = 0;
                        }
                    }
                    land
                }
                (None, None) => GrayImage::new(w, h),
            };
            let objects: Vec<&GrayImage> = found.iter().map(|d| &d.mask).collect();
            let (under, line) = ground_line(&sky, &objects, &mut land);
            ground = line;
            if under > 0.0 {
                notes.push(format!("depth layers: {under:.1}% of the picture is ground under the ground line; moved from Far to Land"));
            }
            let moved = ground_islands(&sky, &mut land);
            if moved > 0.0 {
                notes.push(format!("depth layers: {moved:.1}% of the picture is ground cut off from the sky; moved from Far to Land"));
            }
            let mut far = GrayImage::new(w, h);
            for ((f, s), l) in far.iter_mut().zip(sky.iter()).zip(land.iter()) {
                *f = if *s > 127 || *l > 127 { 0 } else { 255 };
            }
            [(Plate::Sky, sky), (Plate::Far, far), (Plate::Land, land)]
        }
        (None, Some([sky, far, land])) => [(Plate::Sky, sky), (Plate::Far, far), (Plate::Land, land)],
        (None, None) => [(Plate::Sky, GrayImage::new(w, h)), (Plate::Far, GrayImage::new(w, h)), (Plate::Land, GrayImage::new(w, h))],
    };
    if indoor {
        notes.push("scene: indoors with no sky; one Room layer behind the objects instead of Sky / Far / Land".into());
    } else {
        let share = |m: &GrayImage| m.iter().filter(|&&v| v > 127).count() as f64 * 100.0 / (w as f64 * h as f64);
        let (sky, far, land) = (share(&plate_masks[0].1), share(&plate_masks[1].1), share(&plate_masks[2].1));
        notes.push(format!(
            "depth layers: Sky {sky:.0}% ({}), Far {far:.0}%, Land {land:.0}% ({})",
            if !by_sam { "colour rule" } else if sky > 0.0 { "SAM 2" } else { "none" },
            if by_sam && plates.is_none() { "SAM 2" } else { "colour rule" }
        ));
    }

    note("Giving each object its shapes");
    // Each shape to the object covering most of it, if that is at least TAKE_MIN. A filled shape
    // not taken whole is cut along **every** object whose reach overlaps it by CUT_OVERLAP_MIN px^2
    // or more (S13; `cut_of`): the cuts are made below, nearest object first.
    // An object's reach is its mask grown a few px, so the outline strokes round its silhouette (half
    // outside the matte) go with it instead of being left under it.
    let reach: Vec<GrayImage> = found.iter().map(|d| GrayImage::from_raw(w, h, buzz_ai::segment::dilate(d.mask.as_raw(), w, h, REACH)).unwrap_or_else(|| d.mask.clone())).collect();
    let reach_box: Vec<Rect> = found
        .iter()
        .map(|d| Rect::new(d.bbox[0] as f64 - REACH as f64, d.bbox[1] as f64 - REACH as f64, d.bbox[2] as f64 + REACH as f64, d.bbox[3] as f64 + REACH as f64))
        .collect();
    let mut owner: Vec<Option<usize>> = Vec::with_capacity(shapes.len());
    // Per object: the filled shapes (indices into `shapes`, paint order) it cuts a piece of.
    let mut cut_of: Vec<Vec<usize>> = vec![Vec::new(); found.len()];
    for (k, o) in shapes.iter().enumerate() {
        let ObjectKind::Shape(sd) = &o.kind else {
            owner.push(None);
            continue;
        };
        let pts = sample_points(sd, to_px * o.transform);
        let mut best: Option<(usize, f64)> = None;
        // An outline stroke runs on beyond the object it edges, so a smaller share is enough.
        let take = if sd.stroke.is_some() { TAKE_MIN_STROKE } else { TAKE_MIN };
        for (i, r) in reach.iter().enumerate() {
            let c = covered(r, &pts);
            if best.is_none_or(|b| c > b.1) {
                best = Some((i, c));
            }
        }
        // A line that runs on far beyond the object (the picture's frame, a long wall edge) is not
        // the object's own, however much of it passes close by: it stays where it is.
        let stays_inside = |i: usize| {
            sd.stroke.is_none() || {
                let mut b = sd.path.clone();
                b.apply_affine(to_px * o.transform);
                let bb = b.bounding_box().inflate(1.0, 1.0);
                bb.intersect(reach_box[i]).area() >= STROKE_INSIDE_MIN * bb.area()
            }
        };
        match best {
            Some((i, c)) if c >= take && stays_inside(i) => owner.push(Some(i)),
            _ => {
                owner.push(None);
                if sd.fill.is_some() && sd.stroke.is_none() {
                    let mut b = sd.path.clone();
                    b.apply_affine(to_px * o.transform);
                    let bb = b.bounding_box();
                    if bb.width() * bb.height() >= CUT_AREA_MIN {
                        for (i, r) in reach.iter().enumerate() {
                            if overlap_area(&b, r, reach_box[i], CUT_OVERLAP_MIN) >= CUT_OVERLAP_MIN {
                                cut_of[i].push(k);
                            }
                        }
                    }
                }
            }
        }
    }

    note("Filling the holes");
    // What an object leaves behind is its mask plus everything its shapes cover
    // (shapes may stick out of the mask), so hiding it shows filler everywhere.
    let mut found = found;
    let sam_masks: Vec<GrayImage> = found.iter().map(|d| d.mask.clone()).collect();
    let mut taken_ids: Vec<Vec<ObjectId>> = vec![Vec::new(); found.len()];
    // Objects nearest first (lower bottom edge), equal bottoms by label then box: the order they cut
    // a shared shape, each from what the objects before it left (a dropped object takes nothing).
    let mut order: Vec<usize> = (0..found.len()).collect();
    order.sort_by(|&a, &b| found[b].bbox[3].cmp(&found[a].bbox[3]).then_with(|| found[a].label.cmp(&found[b].label)).then_with(|| found[a].bbox.cmp(&found[b].bbox)));
    // What is left of each cut shape (its own coordinates) after the kept objects took their pieces.
    let mut remain: Vec<Option<BezPath>> = vec![None; shapes.len()];
    let mut cuts: Vec<Vec<Cut>> = found.iter().map(|_| Vec::new()).collect();
    // The shape (index into `shapes`) behind each cut of each object.
    let mut cut_k: Vec<Vec<usize>> = found.iter().map(|_| Vec::new()).collect();
    let mut cut_shapes: Vec<Vec<Arc<Object>>> = found.iter().map(|_| Vec::new()).collect();
    // One renderer for every object's "shapes alone" picture (started on the first).
    let mut painter: Option<buzz_export::Exporter> = None;
    // Whole shapes of the objects dropped below (they sit inside another object).
    let mut orphans: Vec<ObjectId> = Vec::new();
    // Objects dropped by the painted check below (already reported there).
    let mut dropped = vec![false; found.len()];
    // What each kept object paints on its own: its outline goes round this.
    let mut silhouettes: Vec<Option<GrayImage>> = vec![None; found.len()];
    for &i in &order {
        let d = &mut found[i];
        let ids: Vec<ObjectId> = shapes.iter().zip(&owner).filter(|(_, o)| **o == Some(i)).map(|(s, _)| s.id).collect();
        // The pieces this object would cut off, each from what the objects before it left.
        let mut left_after: Vec<(usize, BezPath)> = Vec::new();
        if !cut_of[i].is_empty() {
            let outline = mask_outline(&d.mask, opts);
            let grown = GrayImage::from_raw(w, h, buzz_ai::segment::dilate(d.mask.as_raw(), w, h, SEAM_OVERLAP)).unwrap_or_else(|| d.mask.clone());
            let outline_in = mask_outline(&grown, opts);
            for &k in &cut_of[i] {
                let o = &shapes[k];
                let ObjectKind::Shape(sd) = &o.kind else { continue };
                let m = to_px * o.transform;
                let det = m.determinant();
                if det.abs() < 1e-12 || !det.is_finite() || outline.elements().is_empty() {
                    continue;
                }
                let cur = remain[k].as_ref().unwrap_or(&sd.path);
                if cur.elements().is_empty() {
                    continue;
                }
                let mut local = outline.clone();
                local.apply_affine(m.inverse());
                let mut local_in = outline_in.clone();
                local_in.apply_affine(m.inverse());
                let bo = buzz_geom::boolean::options_for(&[cur.clone(), local.clone(), local_in.clone()]).fast();
                let inside = buzz_geom::boolean::boolean(cur, &local_in, buzz_geom::boolean::BoolOp::Intersect, bo);
                if inside.elements().is_empty() {
                    continue;
                }
                let rest = buzz_geom::boolean::boolean(cur, &local, buzz_geom::boolean::BoolOp::Difference, bo);
                let inside = ShapeData { path: inside, ..sd.clone() };
                cut_shapes[i].push(Arc::new(Object::shape(o.id, inside.clone()).with_transform(o.transform)));
                cuts[i].push(Cut { id: o.id, inside, rest: None });
                cut_k[i].push(k);
                left_after.push((k, rest));
            }
        }
        if !ids.is_empty() || !cuts[i].is_empty() {
            let mut mine: Vec<_> = shapes.iter().filter(|s| ids.contains(&s.id) || cut_shapes[i].iter().any(|c| c.id == s.id)).cloned().collect();
            for m in &mut mine {
                if let Some(c) = cut_shapes[i].iter().find(|c| c.id == m.id) {
                    *m = c.clone();
                }
            }
            let ex = match &mut painter {
                Some(ex) => ex,
                None => painter.insert(buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).map_err(|e| format!("no renderer: {e}"))?),
            };
            let alone = render_on(ex, scene, layer, frame, &mine, region, (w, h), true)?;
            let (mut inside, mut painted) = (0u64, 0u64);
            for (m, p) in d.mask.iter().zip(alone.pixels()) {
                if *m > 127 {
                    inside += 1;
                    painted += (p[3] > 8) as u64;
                }
            }
            if (painted as f64) < PAINTED_MIN * inside as f64 {
                notes.push(format!("objects: {} {:?} skipped (its shapes paint only {:.0}% of its mask; it sits inside another object)", d.label, d.bbox, painted as f64 * 100.0 / inside.max(1) as f64));
                cuts[i].clear();
                cut_k[i].clear();
                dropped[i] = true;
                orphans.extend(ids);
                continue;
            }
            silhouettes[i] = Some(GrayImage::from_fn(w, h, |x, y| image::Luma([if alone.get_pixel(x, y)[3] > 127 { 255 } else { 0 }])));
            for (m, p) in d.mask.iter_mut().zip(alone.pixels()) {
                if p[3] > 8 {
                    *m = 255;
                }
            }
            for (k, rest) in left_after {
                remain[k] = Some(rest);
            }
        }
        taken_ids[i] = ids;
    }
    drop(painter);
    // A dropped object's shapes go to the kept object covering most of them (as many as the first
    // pass would have given it), so they stay above the fill pieces cut from under them instead of
    // falling onto a depth layer behind those pieces.
    if !orphans.is_empty() {
        let pos: std::collections::HashMap<ObjectId, usize> = shapes.iter().enumerate().map(|(k, s)| (s.id, k)).collect();
        for id in orphans {
            let Some(o) = pos.get(&id).map(|&k| &shapes[k]) else { continue };
            let ObjectKind::Shape(sd) = &o.kind else { continue };
            let pts = sample_points(sd, to_px * o.transform);
            let take = if sd.stroke.is_some() { TAKE_MIN_STROKE } else { TAKE_MIN };
            let mut best: Option<(usize, f64)> = None;
            for (j, r) in reach.iter().enumerate() {
                if taken_ids[j].is_empty() && cuts[j].is_empty() {
                    continue;
                }
                let c = covered(r, &pts);
                if best.is_none_or(|b| c > b.1) {
                    best = Some((j, c));
                }
            }
            if let Some((j, _)) = best.filter(|b| b.1 >= take) {
                taken_ids[j].push(id);
            }
        }
        for ids in &mut taken_ids {
            ids.sort_by_key(|id| pos.get(id).copied().unwrap_or(usize::MAX));
        }
    }
    // The source keeps one remainder per cut shape, whichever objects cut it.
    for (cs, ks) in cuts.iter_mut().zip(&cut_k) {
        for (c, &k) in cs.iter_mut().zip(ks) {
            let ObjectKind::Shape(sd) = &shapes[k].kind else { continue };
            c.rest = remain[k].as_ref().filter(|p| !p.elements().is_empty()).map(|p| ShapeData { path: p.clone(), ..sd.clone() });
        }
    }
    let mut union = GrayImage::new(w, h);
    for ((d, ids), cs) in found.iter().zip(&taken_ids).zip(&cuts) {
        if ids.is_empty() && cs.is_empty() {
            continue;
        }
        for (u, m) in union.iter_mut().zip(d.mask.iter()) {
            if *m > 127 {
                *u = 255;
            }
        }
    }
    // A hole's colour is read from round it, never from another object or the sky.
    let mut exclude = union;
    for (e, s) in exclude.iter_mut().zip(plate_masks[0].1.iter()) {
        if *s > 127 {
            *e = 255;
        }
    }
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut objects: Vec<ArtworkObject> = Vec::new();
    // Every shape no object keeps goes to the depth layer covering most of it (Land when none).
    let mut kept: std::collections::HashSet<ObjectId> = Default::default();
    for ids in &taken_ids {
        kept.extend(ids.iter().copied());
    }
    let mut plate_ids: [Vec<ObjectId>; 3] = Default::default();
    let mut plate_cuts: [Vec<Cut>; 3] = Default::default();
    // The Sky layer's base: the fitted ramp and glows over the whole frame (the sky's own pieces sit
    // on it); made here because the plates' pieces are read against it (S18).
    let sky_fit: Option<Vec<ShapeData>> = (by_sam && plate_masks[0].1.iter().any(|&v| v > 127)).then(|| sky_base_of(&picture, &plate_masks[0].1));
    let mut removed: Vec<ObjectId> = Vec::new();
    if by_sam {
        // By the plate masks, cutting the shapes that straddle them (S16).
        (plate_ids, plate_cuts) = assign_to_plates(&shapes, &kept, &remain, &plate_masks, to_px, opts);
        if let Some(fit) = &sky_fit {
            let fitted = sky_by_fit(&shapes, &remain, &plate_masks[0].1, &ground, fit, to_px, &mut plate_ids, &mut plate_cuts);
            removed = fitted.removed;
            let counts = fitted.counts;
            if counts.iter().any(|&c| c > 0) {
                notes.push(format!(
                    "depth layers: the sky is drawn by the fit: {} piece(s) removed from Sky, {} moved to Far (tree tips), {} sky-coloured piece(s) removed from Far",
                    counts[0], counts[1], counts[2]
                ));
            }
            if fitted.opened[0] > 0 {
                notes.push(format!(
                    "depth layers: the sky is opened in {} texture(s) on Sky and Far: {:.1}% of their texels ({} of {}) see-through",
                    fitted.opened[0],
                    fitted.opened[1] as f64 * 100.0 / fitted.opened[2].max(1) as f64,
                    fitted.opened[1],
                    fitted.opened[2]
                ));
            }
        }
    } else {
        // (shape id, depth layer, box in bitmap pixels), in paint order.
        let mut left: Vec<(ObjectId, usize, Rect)> = Vec::new();
        for o in shapes.iter() {
            let ObjectKind::Shape(sd) = &o.kind else { continue };
            if kept.contains(&o.id) {
                continue;
            }
            let pts = sample_points(sd, to_px * o.transform);
            let mut best = (2usize, 0.0f64);
            for (k, (_, m)) in plate_masks.iter().enumerate() {
                let c = covered(m, &pts);
                if c > best.1 {
                    best = (k, c);
                }
            }
            let mut path = sd.path.clone();
            path.apply_affine(to_px * o.transform);
            left.push((o.id, best.0, path.bounding_box()));
        }
        // Keep the paint order: a shape painted after one that overlaps it must not end up on a layer
        // behind it, so it moves up to the nearest layer of the shapes it overlaps.
        for i in 0..left.len() {
            let mut k = left[i].1;
            for j in 0..i {
                if left[j].1 > k && left[j].2.intersect(left[i].2).area() > 0.0 {
                    k = left[j].1;
                }
            }
            left[i].1 = k;
            plate_ids[k].push(left[i].0);
        }
    }
    // Each kept object's silhouette (what its shapes paint), smoothed and with its small holes
    // filled, gives it its bold outline and the filler of the hole it leaves: one flat shape in
    // the colour round it, none where the sky is behind it (the Sky layer shows there).
    struct Crop {
        label: String,
        bbox: [u32; 4],
        sam: GrayImage,
        shape_ids: Vec<ObjectId>,
        cuts: Vec<Cut>,
        filler: Vec<ShapeData>,
        outline: Option<ShapeData>,
    }
    let (line_colour, line_px) = line_style(&shapes, to_px);
    let mut crops: Vec<Crop> = Vec::new();
    for (((((d, shape_ids), cuts), sam), dropped), silhouette) in found.into_iter().zip(taken_ids).zip(cuts).zip(sam_masks).zip(dropped).zip(silhouettes) {
        if shape_ids.is_empty() && cuts.is_empty() {
            if !dropped {
                notes.push(format!("objects: {} {:?} skipped (no shape is mostly inside it)", d.label, d.bbox));
            }
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
        for (x, y, p) in d.mask.enumerate_pixels() {
            if p[0] > 127 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
        let silhouette = silhouette.unwrap_or_else(|| d.mask.clone());
        let area = silhouette.iter().filter(|&&v| v > 127).count();
        let silhouette = tidy_mask(&smooth_mask(&silhouette, SILHOUETTE_SMOOTH), (area as f64 * SILHOUETTE_HOLE_MAX) as usize, SILHOUETTE_ISLAND_MIN);
        let mut hole = GrayImage::from_raw(w, h, buzz_ai::segment::dilate(silhouette.as_raw(), w, h, 2)).unwrap_or_else(|| silhouette.clone());
        for (o, s) in hole.iter_mut().zip(plate_masks[0].1.iter()) {
            if *s > 127 {
                *o = 0;
            }
        }
        let filler = if hole.iter().any(|&v| v > 127) {
            let colour = ring_colour(&picture, &hole, &exclude);
            let path = mask_outline(&hole, opts);
            if path.elements().is_empty() { Vec::new() } else { vec![ShapeData::filled(path, colour)] }
        } else {
            Vec::new()
        };
        let outline = Some(mask_outline(&silhouette, opts))
            .filter(|path| !path.elements().is_empty())
            .map(|path| ShapeData::stroked(path, line_colour, line_px));
        crops.push(Crop { label: d.label, bbox: [x0, y0, x1, y1], sam, shape_ids, cuts, filler, outline });
    }
    // Without SAM 2 (colour rule): the sky carried straight down (and up) its columns, so the sky
    // base reads as more sky.
    let sky_base: Option<RgbaImage> = plates.as_ref().filter(|p| !by_sam && p.sky.pixels().any(|p| p[3] > 128)).map(|plates| {
        let mut sky = plates.sky.clone();
        let (wu, hu) = (w as usize, h as usize);
        let mut col: Vec<Option<(usize, usize)>> = vec![None; wu];
        for x in 0..wu {
            let rows: Vec<usize> = (0..hu).filter(|&y| sky.get_pixel(x as u32, y as u32)[3] > 128).collect();
            if let (Some(&a), Some(&b)) = (rows.first(), rows.last()) {
                col[x] = Some((a, b));
            }
        }
        for x in 0..wu {
            let src = (0..wu).filter(|&k| col[k].is_some()).min_by_key(|&k| k.abs_diff(x));
            let Some(sx) = src else { break };
            let (a, b) = col[sx].unwrap_or((0, 0));
            for y in 0..hu {
                if sky.get_pixel(x as u32, y as u32)[3] <= 128 {
                    let sy = if y < a {
                        a
                    } else if y > b {
                        b
                    } else {
                        (a..y).rev().find(|&k| sky.get_pixel(sx as u32, k as u32)[3] > 128).unwrap_or(a)
                    };
                    let mut px = *sky.get_pixel(sx as u32, sy as u32);
                    px[3] = 255;
                    sky.put_pixel(x as u32, y as u32, px);
                }
            }
        }
        sky
    });
    let mut sky_filler = if by_sam {
        // The fitted ramp and glows over the whole frame; the sky's own pieces sit on it.
        sky_fit
    } else {
        // The sky base keeps the sky's own pixels where they are textured or graded (T16b).
        let mut sky_options = opts.trace.trace_options();
        sky_options.keep_textures = true;
        sky_base.as_ref().map(|sky| buzz_scene::trace(w, h, sky.as_raw(), &sky_options).shapes)
    };
    for c in crops {
        let behind = match &plates {
            Some(plates) if !by_sam => {
                if crate::bg_objects::plate_at_bottom(plates, &c.sam) == crate::bg_objects::PlateId::Far {
                    Plate::Far
                } else {
                    Plate::Land
                }
            }
            _ => behind_of(&plate_masks[2].1, &c.sam),
        };
        objects.push(ArtworkObject {
            plate: None,
            behind,
            label: c.label,
            n: 0,
            bbox: c.bbox,
            mask: c.sam,
            shape_ids: c.shape_ids,
            cuts: c.cuts,
            filler: c.filler,
            filler_at: to_world,
            outline: c.outline,
        });
    }
    // Nearest first (lower bottom edge): `apply` puts each new layer right above the source, so
    // the first made ends up topmost and the farthest lowest.
    // Equal bottoms in a fixed order (label, box), so the numbering never depends on the order found.
    // Indoors, a thing standing on another (a kettle on the counter) sits inside that one's box but
    // ends higher up the picture: it counts as just nearer than what holds it, so its layer is in front.
    let front: Vec<u32> = {
        let area = |b: [u32; 4]| (b[2] - b[0]) as f64 * (b[3] - b[1]) as f64;
        let mut front: Vec<u32> = objects.iter().map(|o| o.bbox[3]).collect();
        if indoor {
            for _ in 0..objects.len() {
                let mut changed = false;
                for (i, o) in objects.iter().enumerate() {
                    for (j, c) in objects.iter().enumerate() {
                        let (a, b) = (o.bbox, c.bbox);
                        let ix = a[2].min(b[2]).saturating_sub(a[0].max(b[0])) as f64;
                        let iy = a[3].min(b[3]).saturating_sub(a[1].max(b[1])) as f64;
                        if i != j && area(b) > area(a) && ix * iy >= 0.8 * area(a) && front[i] <= front[j] {
                            front[i] = front[j] + 1;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
        }
        front
    };
    let mut order: Vec<usize> = (0..objects.len()).collect();
    order.sort_by(|&i, &j| front[j].cmp(&front[i]).then_with(|| objects[i].label.cmp(&objects[j].label)).then_with(|| objects[i].bbox.cmp(&objects[j].bbox)));
    let mut slots: Vec<Option<ArtworkObject>> = objects.into_iter().map(Some).collect();
    let mut objects: Vec<ArtworkObject> = order.into_iter().filter_map(|i| slots[i].take()).collect();
    for o in &mut objects {
        o.n = *counts.entry(o.label.clone()).and_modify(|c| *c += 1).or_insert(1);
    }
    // Then the depth layers, nearest first (Land, Far, Sky): each is made right above the source
    // too, so the sky ends up lowest. Sky gets a full-frame base (its colour carried into the
    // empty rest) so hiding anything above it never shows a hole.
    for (k, plate) in [(2usize, Plate::Land), (1, Plate::Far), (0, Plate::Sky)] {
        let under = objects.iter().any(|o| o.plate.is_none() && o.behind == plate);
        let empty = plate_ids[k].is_empty() && plate_cuts[k].is_empty();
        if empty && !under && plate != Plate::Sky {
            continue;
        }
        let mut filler = Vec::new();
        if plate == Plate::Sky && sky_filler.is_some() {
            filler = sky_filler.take().unwrap_or_default();
        } else if plate == Plate::Sky && empty {
            continue;
        }
        let name = match plate {
            Plate::Sky => "Sky",
            Plate::Far => "Far",
            Plate::Land if indoor => "Room",
            Plate::Land => "Land",
        };
        objects.push(ArtworkObject {
            plate: Some(plate),
            behind: plate,
            label: name.into(),
            n: 0,
            bbox: [0, 0, w, h],
            mask: plate_masks[k].1.clone(),
            shape_ids: std::mem::take(&mut plate_ids[k]),
            cuts: std::mem::take(&mut plate_cuts[k]),
            filler,
            filler_at: to_world,
            outline: None,
        });
    }
    Ok(ArtworkPlan { layer, frame, objects, notes, picture, to_world, record, removed })
}

/// The run record's `result.json` for a finished artwork split: the layers `apply` made
/// (`made`, front to back) listed back to front with their box and the shapes they took
/// (`shapes` = whole + cut; `shape_ids` the whole ones in paint order), the object boxes
/// (bitmap pixels) and the notes.
pub fn result_json(plan: &ArtworkPlan, made: &[String]) -> serde_json::Value {
    let name_of = |o: &ArtworkObject| if o.plate.is_some() { o.label.clone() } else { format!("{} {}", o.label, o.n) };
    let layers: Vec<serde_json::Value> = made
        .iter()
        .rev()
        .map(|name| match plan.objects.iter().find(|o| &name_of(o) == name) {
            Some(o) => serde_json::json!({
                "name": name,
                "depth_layer": o.plate.is_some(),
                "bbox": o.bbox,
                "shapes": o.shape_ids.len() + o.cuts.len(),
                "shape_ids": o.shape_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
                "cut_ids": o.cuts.iter().map(|c| c.id.0).collect::<Vec<_>>(),
                "filler_shapes": o.filler.len(),
            }),
            None => serde_json::json!({ "name": name }),
        })
        .collect();
    serde_json::json!({
        "route": "artwork",
        "layers_back_to_front": made.iter().rev().collect::<Vec<_>>(),
        "layers": layers,
        "objects": plan.objects.iter().filter(|o| o.plate.is_none()).map(|o| serde_json::json!({ "name": name_of(o), "label": o.label, "n": o.n, "bbox": o.bbox })).collect::<Vec<_>>(),
        "notes": plan.notes,
    })
}

/// The keyframe of `layer` governing `frame`: its first frame and the frame after its last.
pub fn keyframe_span(scene: &Scene, layer: LayerId, frame: u32) -> Option<(u32, u32)> {
    let frames = &scene.layers().get(layer)?.frames;
    let start = frames.keyframe_start(frame)?;
    let end = frames.keyframes().iter().map(|k| k.start).find(|&k| k > start).unwrap_or(frames.length());
    Some((start, end))
}

/// Take `id` off the keyframe starting at `start` only. A keyframe made with F6 shares its
/// objects, ids and all, with the one it was made from, so a remove by id alone could empty
/// the first keyframe holding it instead of the one being split.
fn take_at(scene: &mut Scene, layer: LayerId, start: u32, id: ObjectId) -> Option<Arc<Object>> {
    let object = scene.layers().get(layer)?.frames.objects_at(start).iter().find(|o| o.id == id)?.clone();
    scene.remove_object_where(buzz_scene::EditAt::exact(start), id).then_some(object)
}

/// Carry the plan out on `scene` (inside one `Document::edit`): per object one symbol,
/// one layer above the source, the filler behind the source's remaining shapes.
/// Only the keyframe at `plan.frame` is split: the new layers show their objects over that
/// keyframe's frames and are blank before and after it, and the source's other keyframes
/// keep their artwork. Returns the names of the layers made.
pub fn apply(scene: &mut Scene, plan: &ArtworkPlan) -> Vec<String> {
    let layer = plan.layer;
    let folder = scene.layers().get(layer).and_then(|l| l.parent);
    let span = scene.layers().get(layer).map_or(1, |l| l.frames.length());
    let (start, end) = keyframe_span(scene, layer, plan.frame).unwrap_or((0, span));
    let mut made = Vec::new();
    // Object fillers go into the depth layer behind them, under that layer's shapes.
    let mut under: [Vec<Arc<Object>>; 3] = Default::default();
    for o in plan.objects.iter().filter(|o| o.plate.is_none()) {
        let mut filler = o.filler.clone();
        buzz_scene::adopt_texture_images(&mut filler, scene);
        for sd in &filler {
            under[o.behind as usize].push(Arc::new(Object::shape(scene.next_object_id(), sd.clone()).with_transform(o.filler_at)));
        }
    }
    // The source's paint order, and the shapes several objects may cut, as they were before any
    // object took anything (the first object to cut a shape may remove it).
    let order: Vec<ObjectId> = scene.layers().get(layer).map(|l| l.frames.objects_at(plan.frame).iter().map(|x| x.id).collect()).unwrap_or_default();
    let cut_origs: std::collections::HashMap<ObjectId, Arc<Object>> = scene
        .layers()
        .get(layer)
        .map(|l| l.frames.objects_at(plan.frame).iter().filter(|x| plan.objects.iter().any(|o| o.cuts.iter().any(|c| c.id == x.id))).map(|x| (x.id, x.clone())).collect())
        .unwrap_or_default();
    // The cut pieces, with the Far textures that have the sky opened (S18b, a copy under a
    // placeholder id) put in the library together: pieces sharing a copy keep sharing it.
    let mut insides: Vec<ShapeData> = plan.objects.iter().flat_map(|o| o.cuts.iter().map(|c| c.inside.clone())).collect();
    buzz_scene::adopt_texture_images(&mut insides, scene);
    let mut insides = insides.into_iter();
    for o in &plan.objects {
        // Whole shapes and cut-off inside pieces, in the source's paint order.
        let at = |id: &ObjectId| order.iter().position(|x| x == id).unwrap_or(usize::MAX);
        let mut taken: Vec<(usize, Arc<Object>)> = o.shape_ids.iter().filter_map(|id| take_at(scene, layer, start, *id).map(|x| (at(id), x))).collect();
        for c in &o.cuts {
            let inside = insides.next().unwrap_or_else(|| c.inside.clone());
            let Some(orig) = cut_origs.get(&c.id).cloned() else { continue };
            let mut piece = Object::shape(scene.next_object_id(), inside).with_transform(orig.transform);
            // The cut-off piece keeps what the shape wore besides its path (a soft edge's blur).
            piece.filters = orig.filters.clone();
            let piece = Arc::new(piece);
            taken.push((at(&c.id), piece));
            match &c.rest {
                Some(rest) => {
                    scene.update_object_at(start, c.id, |x| {
                        if let ObjectKind::Shape(sd) = &mut x.kind {
                            *sd = rest.clone();
                        }
                    });
                }
                None => {
                    take_at(scene, layer, start, c.id);
                }
            }
        }
        taken.sort_by_key(|t| t.0);
        // A shape the source's other keyframes still hold goes into the symbol under a new id,
        // so no id is in two places.
        let mut taken: Vec<Arc<Object>> = taken
            .into_iter()
            .map(|(_, x)| {
                if scene.layers().get(layer).is_some_and(|l| l.find_object(x.id).is_some()) {
                    let mut copy = (*x).clone();
                    copy.id = scene.next_object_id();
                    Arc::new(copy)
                } else {
                    x
                }
            })
            .collect();
        if o.plate.is_some() {
            // The layer's base first (behind), then the fillers of the objects above it.
            let mut filler = o.filler.clone();
            buzz_scene::adopt_texture_images(&mut filler, scene);
            let mut base: Vec<Arc<Object>> = filler.iter().map(|sd| Arc::new(Object::shape(scene.next_object_id(), sd.clone()).with_transform(o.filler_at))).collect();
            base.append(&mut under[o.behind as usize]);
            base.append(&mut taken);
            taken = base;
        }
        if taken.is_empty() {
            continue;
        }
        if let Some(line) = &o.outline {
            taken.push(Arc::new(Object::shape(scene.next_object_id(), line.clone()).with_transform(plan.to_world)));
        }
        let name = if o.plate.is_some() { o.label.clone() } else { format!("{} {}", o.label, o.n) };
        let symbol = scene.add_symbol(name.clone(), SymbolKind::Graphic, Some("Split objects"));
        let Some(inner) = scene.library().get(symbol).and_then(|s| s.layers.iter().next()).map(|l| l.id) else { continue };
        scene.library_mut().update(symbol, |sym| {
            sym.layers.update(inner, |l| {
                l.frames.set_objects(0, taken);
            });
        });
        let new = scene.add_layer(name.clone(), LayerKind::Normal);
        // Directly above the source layer.
        if let Some(i) = scene.layer_index(layer) {
            scene.reorder_layer(new, i.saturating_sub(1));
        }
        scene.update_layer(new, |l| {
            l.parent = folder;
            if start > 0 {
                l.frames.insert_blank_keyframe(start);
            }
        });
        scene.add_instance_at(new, start, symbol, Affine::IDENTITY);
        scene.update_layer(new, |l| {
            if l.frames.length() < span {
                l.frames.insert_frame(span - 1);
            }
            if end < span {
                l.frames.insert_blank_keyframe(end);
            }
        });
        made.push(name);
    }
    // What the fitted sky draws instead (S18).
    for id in &plan.removed {
        take_at(scene, layer, start, *id);
    }
    // Nothing left on the source layer: remove it when empty.
    if scene.layers().get(layer).is_some_and(|l| l.frames.keyframes().iter().all(|k| k.objects.is_empty())) {
        scene.remove_layer(layer);
    }
    made
}

/// The colour and width (bitmap px) of the bold outline round each object cut out of the
/// artwork: its most used stroke colour and its boldest common width (the 90th percentile), never
/// thinner than [`OUTLINE_WIDTH`]; near-black when it has no strokes (a trace whose ink is filled
/// shapes).
fn line_style(shapes: &[Arc<Object>], to_px: Affine) -> (peniko::Color, f64) {
    let scale = |m: Affine| m.determinant().abs().sqrt();
    let mut colours: std::collections::BTreeMap<[u8; 4], usize> = Default::default();
    let mut widths: Vec<f64> = Vec::new();
    for o in shapes {
        let ObjectKind::Shape(sd) = &o.kind else { continue };
        let Some(stroke) = &sd.stroke else { continue };
        let buzz_scene::Paint::Solid(colour) = stroke.paint else { continue };
        if stroke.width > 0.0 {
            *colours.entry(colour.to_rgba8().to_u8_array()).or_default() += 1;
            widths.push(stroke.width * scale(o.transform) * scale(to_px));
        }
    }
    widths.sort_by(f64::total_cmp);
    let width = widths.get(widths.len() * 9 / 10).copied().unwrap_or(0.0).max(OUTLINE_WIDTH);
    match colours.into_iter().max_by_key(|(c, n)| (*n, *c)) {
        Some(([r, g, b, _], _)) => (peniko::Color::from_rgb8(r, g, b), width),
        None => (peniko::Color::from_rgb8(16, 16, 16), width),
    }
}

/// The outline of `mask` (bitmap pixels) as one path of smooth curves, traced from its bounding
/// box. Its own trace settings, whatever the caller's look: filled regions only (a bold trace's
/// outline strokes would add centre-lines to the path), the pixel staircase taken out
/// ([`OUTLINE_DETAIL`]) and the corners rounded ([`OUTLINE_SMOOTH`]), so neither a cut along it nor
/// a line drawn on it looks torn.
fn mask_outline(mask: &GrayImage, _opts: &ObjectOptions) -> BezPath {
    let (w, h) = mask.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for (x, y, p) in mask.enumerate_pixels() {
        if p[0] > 127 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    let mut out = BezPath::new();
    if x1 <= x0 || y1 <= y0 {
        return out;
    }
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut px = vec![0u8; (cw * ch * 4) as usize];
    for y in 0..ch {
        for x in 0..cw {
            if mask.get_pixel(x0 + x, y0 + y)[0] > 127 {
                px[((y * cw + x) * 4 + 3) as usize] = 255;
            }
        }
    }
    let t = buzz_scene::TraceOptions {
        mode: buzz_scene::TraceMode::Outline,
        colours: 2,
        detail: OUTLINE_DETAIL,
        smooth: OUTLINE_SMOOTH,
        speckle: 16,
        drop_background: false,
        ..buzz_scene::TraceOptions::default()
    };
    for sd in buzz_scene::trace(cw, ch, &px, &t).shapes.into_iter().filter(|sd| sd.fill.is_some()) {
        out.extend(sd.path.iter());
    }
    out.apply_affine(Affine::translate((x0 as f64, y0 as f64)));
    out
}

// -- remove from picture (S7) and the shared patch (S7b) -----------------------------------

/// How far (px) the hole of a removal grows before it is inpainted.
const PATCH_GROW: i32 = 4;
/// A hole component smaller than this (px) is left to the colour fill.
const PATCH_MIN: usize = 16;

/// What `patch_hole` did with the pieces of the hole (S14: so the status says why LaMa was not used).
#[derive(Default, Clone, Copy, Debug)]
pub struct PatchReport {
    /// Pieces LaMa filled.
    pub lama: usize,
    /// Pieces left to the colour fill because the LaMa model is not installed.
    pub not_installed: usize,
    /// Pieces left to the colour fill because LaMa failed.
    pub failed: usize,
    /// Pieces left to the colour fill because they are tiny (or no window fits them).
    pub skipped: usize,
}

/// How LaMa went on one piece.
enum Piece {
    Done,
    NotInstalled,
    Failed,
    Skipped,
}

/// Patch `hole` (255 = hole, same size as `img`) of `img` with **LaMa**: each connected piece of the
/// hole, grown by `grow` px, is cropped with at least half its size of context, resized to 512,
/// inpainted, resized back and pasted inside the hole only. Pieces LaMa cannot do (not installed,
/// failed, tiny) get `bg_objects::fill_hole`. Returns what happened to the pieces. Alpha is 255 in the hole after.
pub fn patch_hole(img: &mut RgbaImage, hole: &GrayImage, grow: i32) -> PatchReport {
    let mut report = PatchReport::default();
    let (w, h) = img.dimensions();
    if hole.dimensions() != (w, h) {
        return report;
    }
    let grown = buzz_ai::segment::dilate(hole.as_raw(), w, h, grow.max(0));
    // 8-connected pieces of the grown hole: pixel indices and bounding box.
    let mut seen = vec![false; (w * h) as usize];
    let mut pieces: Vec<(Vec<usize>, [u32; 4])> = Vec::new();
    for s in 0..(w * h) as usize {
        if grown[s] == 0 || seen[s] {
            continue;
        }
        let mut px = Vec::new();
        let mut bb = [w, h, 0u32, 0u32];
        let mut stack = vec![s];
        seen[s] = true;
        while let Some(i) = stack.pop() {
            px.push(i);
            let (x, y) = ((i as u32) % w, (i as u32) / w);
            bb = [bb[0].min(x), bb[1].min(y), bb[2].max(x + 1), bb[3].max(y + 1)];
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = (ny as u32 * w + nx as u32) as usize;
                    if grown[j] > 0 && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        pieces.push((px, bb));
    }
    let mut pending = grown.clone();
    let mut left = GrayImage::new(w, h);
    for (px, bb) in pieces {
        let done = if px.len() >= PATCH_MIN { lama_piece(img, &pending, &px, bb) } else { Piece::Skipped };
        match done {
            Piece::Done => {
                report.lama += 1;
                for &i in &px {
                    pending[i] = 0;
                }
                continue;
            }
            Piece::NotInstalled => report.not_installed += 1,
            Piece::Failed => report.failed += 1,
            Piece::Skipped => report.skipped += 1,
        }
        for &i in &px {
            left.as_mut()[i] = 255;
        }
    }
    if left.iter().any(|v| *v > 0) {
        crate::bg_objects::fill_hole(img, &left, 0);
    }
    report
}

/// LaMa over one piece (`px` = its pixel indices, `bb` its box); `pending` is every hole pixel not done yet.
/// The square window (twice the piece, at least 128 px, at most twice the picture's longer side) may be
/// bigger than the picture: what lies outside it is the picture mirrored at its edge (holes included), and only
/// the piece's own pixels are pasted back.
fn lama_piece(img: &mut RgbaImage, pending: &[u8], px: &[usize], bb: [u32; 4]) -> Piece {
    let (w, h) = img.dimensions();
    let extent = (bb[2] - bb[0]).max(bb[3] - bb[1]);
    let side = (extent * 2).max(128).min(2 * w.max(h));
    if side < 8 {
        return Piece::Skipped;
    }
    let (cx, cy) = ((bb[0] + bb[2]) / 2, (bb[1] + bb[3]) / 2);
    // The window's first column / row: inside the picture when it fits, else centred (negative).
    let start = |c: u32, dim: u32| -> i64 { if side <= dim { c.saturating_sub(side / 2).min(dim - side) as i64 } else { -(((side - dim) / 2) as i64) } };
    let (x0, y0) = (start(cx, w), start(cy, h));
    let s = side as i64;
    if px.iter().any(|&i| {
        let (x, y) = ((i as u32 % w) as i64, (i as u32 / w) as i64);
        x < x0 || y < y0 || x >= x0 + s || y >= y0 + s
    }) {
        return Piece::Skipped;
    }
    // Reflect a coordinate back into 0..n (symmetric about the edge, repeating).
    let mirror = |v: i64, n: u32| -> u32 {
        let n = n as i64;
        let m = v.rem_euclid(2 * n);
        (if m >= n { 2 * n - 1 - m } else { m }) as u32
    };
    // The window row by row (the mirrored column of every window column found once).
    let cols: Vec<usize> = (0..s).map(|x| mirror(x0 + x, w) as usize).collect();
    let side_u = side as usize;
    let (mut crop_raw, mut mask_raw) = (vec![0u8; side_u * side_u * 4], vec![0u8; side_u * side_u]);
    let src = img.as_raw();
    for y in 0..side_u {
        let py = y0 + y as i64;
        let row = mirror(py, h) as usize * w as usize;
        for (x, &sx) in cols.iter().enumerate() {
            crop_raw[(y * side_u + x) * 4..(y * side_u + x) * 4 + 4].copy_from_slice(&src[(row + sx) * 4..(row + sx) * 4 + 4]);
            // A mirrored hole is hole too, so LaMa never sees the objects reflected past the edge.
            if pending[row + sx] > 0 {
                mask_raw[y * side_u + x] = 255;
            }
        }
    }
    let (Some(crop), Some(mask)) = (RgbaImage::from_raw(side, side, crop_raw), GrayImage::from_raw(side, side, mask_raw)) else {
        return Piece::Failed;
    };
    let small = image::imageops::resize(&crop, 512, 512, image::imageops::FilterType::Triangle);
    let small_mask = image::imageops::resize(&mask, 512, 512, image::imageops::FilterType::Nearest);
    let filled = match buzz_ai::segment::lama_inpaint_512(&small, &small_mask) {
        Ok(Some(f)) => f,
        Ok(None) => return Piece::NotInstalled,
        Err(e) => {
            tracing::warn!("LaMa inpainting failed: {e}");
            return Piece::Failed;
        }
    };
    // Only the piece's pixels are read back from the 512 result (bilinear, as the resize would).
    let at = |c: i64| -> (usize, usize, f32) {
        let f = (c as f64 + 0.5) * 512.0 / side as f64 - 0.5;
        let i = f.floor();
        ((i as i64).clamp(0, 511) as usize, (i as i64 + 1).clamp(0, 511) as usize, (f - i) as f32)
    };
    for &i in px {
        let (x, y) = ((i as u32 % w) as i64, (i as u32 / w) as i64);
        let ((xa, xb, tx), (ya, yb, ty)) = (at(x - x0), at(y - y0));
        let mut rgb = [0u8; 3];
        for (c, v) in rgb.iter_mut().enumerate() {
            let g = |xx: usize, yy: usize| filled.get_pixel(xx as u32, yy as u32)[c] as f32;
            let (top, bottom) = (g(xa, ya) * (1.0 - tx) + g(xb, ya) * tx, g(xa, yb) * (1.0 - tx) + g(xb, yb) * tx);
            *v = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
        }
        img.put_pixel(x as u32, y as u32, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
    }
    Piece::Done
}

/// The artwork of one layer rendered once, ready for point prompts (and the removal after them).
pub struct Prepared {
    pub layer: LayerId,
    pub frame: u32,
    pub picture: RgbaImage,
    /// Bitmap pixels -> stage space.
    pub to_world: Affine,
    /// SAM 2's embeddings of `picture`; `None` when only whole shapes are removed.
    pub embeddings: Option<buzz_ai::segment::Sam2Embeddings>,
}

/// Render `layer`'s vector shapes at `frame`, and (with `sam`) encode the picture for SAM 2.
pub fn prepare_remove(scene: &Scene, layer: LayerId, frame: u32, sam: bool) -> Result<Prepared, String> {
    let shapes = source_shapes(scene, layer, frame, None);
    if shapes.is_empty() {
        return Err("Remove from Picture: there are no vector shapes here".into());
    }
    let (picture, to_world) = render_shapes(scene, layer, frame, &shapes)?;
    let embeddings = if sam {
        match buzz_ai::segment::sam2_encode(&picture) {
            Ok(Some(e)) => Some(e),
            Ok(None) => return Err("Remove Object needs the SAM 2 model (AI > Models)".into()),
            Err(e) => return Err(format!("SAM 2 failed: {e}")),
        }
    } else {
        None
    };
    Ok(Prepared { layer, frame, picture, to_world, embeddings })
}

/// SAM 2's mask (0 / 255, picture size) for point prompts in bitmap pixels `(x, y, positive)`.
pub fn mask_at(prep: &Prepared, points: &[buzz_ai::segment::Sam2Point]) -> Result<GrayImage, String> {
    let emb = prep.embeddings.as_ref().ok_or("the picture was not encoded for SAM 2")?;
    match buzz_ai::segment::sam2_mask(emb, points, None) {
        Ok(Some((mut m, _))) => {
            for v in m.iter_mut() {
                *v = if *v > 127 { 255 } else { 0 };
            }
            Ok(m)
        }
        Ok(None) => Err("the SAM 2 decoder is not installed".into()),
        Err(e) => Err(format!("SAM 2 failed: {e}")),
    }
}

/// What to take out of the picture.
pub enum RemoveTarget<'a> {
    /// These shapes, whole.
    Shapes(&'a [ObjectId]),
    /// Whatever the mask covers (picture size, 255 inside): shapes mostly inside go, shapes partly inside are cut.
    Mask(&'a GrayImage),
}

/// What a removal decided; `apply_remove` carries it out.
pub struct RemovePlan {
    pub layer: LayerId,
    pub frame: u32,
    pub remove_ids: Vec<ObjectId>,
    pub cuts: Vec<Cut>,
    /// The patch, in pixels local to `patch_at`'s origin.
    pub patch: Vec<ShapeData>,
    pub patch_at: Affine,
    /// The inpainted region (grown), picture size.
    pub hole: GrayImage,
    pub lama: bool,
}

/// Work out the removal (worker thread): which shapes go or are cut, then the patch.
pub fn plan_remove(scene: &Scene, prep: &Prepared, target: RemoveTarget, opts: &ObjectOptions, note: &mut dyn FnMut(&str)) -> Result<RemovePlan, String> {
    let (layer, frame) = (prep.layer, prep.frame);
    let shapes = source_shapes(scene, layer, frame, None);
    let (w, h) = prep.picture.dimensions();
    let to_px = prep.to_world.inverse();
    note("Choosing the shapes to remove");
    let mut remove_ids: Vec<ObjectId> = Vec::new();
    let mut cuts: Vec<Cut> = Vec::new();
    let mut gone: Vec<Arc<Object>> = Vec::new();
    let mask = match target {
        RemoveTarget::Shapes(ids) => {
            for o in shapes.iter().filter(|o| ids.contains(&o.id)) {
                remove_ids.push(o.id);
                gone.push(o.clone());
            }
            None
        }
        RemoveTarget::Mask(m) => {
            if m.dimensions() != (w, h) {
                return Err("the mask does not fit the picture".into());
            }
            let mut outline: Option<BezPath> = None;
            let on_mask = mask_box(m, 0.0);
            for o in &shapes {
                let ObjectKind::Shape(sd) = &o.kind else { continue };
                let pts = sample_points(sd, to_px * o.transform);
                let c = covered(m, &pts);
                let whole = if sd.fill.is_none() { 0.5 } else { TAKE_MIN };
                if c >= whole {
                    remove_ids.push(o.id);
                    gone.push(o.clone());
                } else if sd.fill.is_some() && sd.stroke.is_none() {
                    let mut b = sd.path.clone();
                    b.apply_affine(to_px * o.transform);
                    let bb = b.bounding_box();
                    // The same rule as the split: a piece of at least CUT_OVERLAP_MIN px^2 is cut off.
                    if bb.width() * bb.height() < CUT_AREA_MIN || overlap_area(&b, m, on_mask, CUT_OVERLAP_MIN) < CUT_OVERLAP_MIN {
                        continue;
                    }
                    let out = outline.get_or_insert_with(|| mask_outline(m, opts)).clone();
                    let mt = to_px * o.transform;
                    let det = mt.determinant();
                    if det.abs() < 1e-12 || !det.is_finite() || out.elements().is_empty() {
                        continue;
                    }
                    let mut local = out;
                    local.apply_affine(mt.inverse());
                    let bo = buzz_geom::boolean::options_for(&[sd.path.clone(), local.clone()]).fast();
                    let inside = buzz_geom::boolean::boolean(&sd.path, &local, buzz_geom::boolean::BoolOp::Intersect, bo);
                    if inside.elements().is_empty() {
                        continue;
                    }
                    let rest = buzz_geom::boolean::boolean(&sd.path, &local, buzz_geom::boolean::BoolOp::Difference, bo);
                    let with = |path: BezPath| ShapeData { path, ..sd.clone() };
                    let inside = with(inside);
                    gone.push(Arc::new(Object::shape(o.id, inside.clone()).with_transform(o.transform)));
                    cuts.push(Cut { id: o.id, inside, rest: (!rest.elements().is_empty()).then(|| with(rest)) });
                }
            }
            Some(m)
        }
    };
    if gone.is_empty() {
        return Err("nothing to remove: no shape of the artwork lies inside that".into());
    }
    // The hole: the mask plus everything the removed shapes (and cut-off pieces) paint.
    note("Finding the hole");
    let region = prep.to_world.transform_rect_bbox(Rect::new(0.0, 0.0, w as f64, h as f64));
    let alone = render_in(scene, layer, frame, &gone, region, (w, h), true)?;
    let mut hole = GrayImage::new(w, h);
    for (x, y, p) in alone.enumerate_pixels() {
        if p[3] > 8 || mask.is_some_and(|m| m.get_pixel(x, y)[0] > 127) {
            hole.put_pixel(x, y, image::Luma([255]));
        }
    }
    note("Patching the hole (LaMa)");
    let mut patched = prep.picture.clone();
    let lama = patch_hole(&mut patched, &hole, PATCH_GROW).lama > 0;
    // Trace the patch over the hole plus a little more, so it sits under the neighbours with no seam.
    let grown = buzz_ai::segment::dilate(hole.as_raw(), w, h, PATCH_GROW + 2);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for (i, v) in grown.iter().enumerate() {
        if *v > 0 {
            let (x, y) = ((i as u32) % w, (i as u32) / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut crop = image::imageops::crop_imm(&patched, x0, y0, cw, ch).to_image();
    for y in 0..ch {
        for x in 0..cw {
            crop.get_pixel_mut(x, y)[3] = if grown[((y0 + y) * w + x0 + x) as usize] > 0 { 255 } else { 0 };
        }
    }
    note("Tracing the patch");
    let patch = buzz_scene::trace(cw, ch, crop.as_raw(), &opts.trace.trace_options()).shapes;
    let hole = GrayImage::from_raw(w, h, buzz_ai::segment::dilate(hole.as_raw(), w, h, PATCH_GROW)).ok_or("bad hole")?;
    Ok(RemovePlan { layer, frame, remove_ids, cuts, patch, patch_at: prep.to_world * Affine::translate((x0 as f64, y0 as f64)), hole, lama })
}

/// Carry the removal out on `scene` (inside one `Document::edit`). Returns how many shapes went or were cut.
pub fn apply_remove(scene: &mut Scene, plan: &RemovePlan) -> usize {
    let mut n = 0;
    for id in &plan.remove_ids {
        n += scene.remove_object(*id).is_some() as usize;
    }
    for c in &plan.cuts {
        let present = scene.layers().get(plan.layer).is_some_and(|l| l.frames.objects_at(plan.frame).iter().any(|x| x.id == c.id));
        if !present {
            continue;
        }
        match &c.rest {
            Some(rest) => {
                scene.update_object(c.id, |x| {
                    if let ObjectKind::Shape(sd) = &mut x.kind {
                        *sd = rest.clone();
                    }
                });
            }
            None => {
                scene.remove_object(c.id);
            }
        }
        n += 1;
    }
    for sd in &plan.patch {
        let patch = Arc::new(Object::shape(scene.next_object_id(), sd.clone()).with_transform(plan.patch_at));
        scene.update_layer(plan.layer, |l| {
            l.frames.insert_object_behind(plan.frame, patch);
        });
    }
    n
}

/// `mask` with its edge smoothed: blurred by `sigma` px and cut at half, which rounds off the
/// staircase and the ragged bits of a mask scaled up from SAM 2's coarse grid.
fn smooth_mask(mask: &GrayImage, sigma: f32) -> GrayImage {
    let blurred = image::imageops::blur(mask, sigma);
    GrayImage::from_fn(mask.width(), mask.height(), |x, y| image::Luma([if blurred.get_pixel(x, y)[0] > 127 { 255 } else { 0 }]))
}

/// `mask` with the enclosed holes smaller than `max_area` px filled, and the islands smaller than
/// `min_island` px dropped: what an object's outline goes round.
fn tidy_mask(mask: &GrayImage, max_area: usize, min_island: usize) -> GrayImage {
    let (w, h) = mask.dimensions();
    let mut out = mask.clone();
    let inverse: Vec<u8> = mask.iter().map(|&v| if v > 127 { 0 } else { 255 }).collect();
    for hole in buzz_ai::segment::connected_components(&inverse, w, h) {
        let on_edge = hole.iter().any(|&i| {
            let (x, y) = ((i as u32) % w, (i as u32) / w);
            x == 0 || y == 0 || x + 1 == w || y + 1 == h
        });
        if !on_edge && hole.len() < max_area {
            for i in hole {
                out.as_mut()[i] = 255;
            }
        }
    }
    for island in buzz_ai::segment::connected_components(out.as_raw(), w, h) {
        if island.len() < min_island {
            for i in island {
                out.as_mut()[i] = 0;
            }
        }
    }
    out
}

/// The colour round `region` in `picture`: the median of the pixels 3 to 9 px outside it that no
/// pixel of `exclude` covers (the other objects, the sky); those taken too when that leaves too
/// few; the region's own colour when nothing is round it.
fn ring_colour(picture: &RgbaImage, region: &GrayImage, exclude: &GrayImage) -> peniko::Color {
    let (w, h) = region.dimensions();
    let near = buzz_ai::segment::dilate(region.as_raw(), w, h, 3);
    let far = buzz_ai::segment::dilate(region.as_raw(), w, h, 9);
    let median = |pick: &dyn Fn(usize) -> bool| -> Option<peniko::Color> {
        let mut ch: [Vec<u8>; 3] = Default::default();
        for i in (0..(w * h) as usize).filter(|&i| pick(i)) {
            let p = picture.as_raw();
            for k in 0..3 {
                ch[k].push(p[i * 4 + k]);
            }
        }
        if ch[0].len() < RING_MIN {
            return None;
        }
        for c in &mut ch {
            c.sort_unstable();
        }
        let m = |c: &Vec<u8>| c[c.len() / 2];
        Some(peniko::Color::from_rgb8(m(&ch[0]), m(&ch[1]), m(&ch[2])))
    };
    let ring = |i: usize| far[i] > 0 && near[i] == 0;
    median(&|i| ring(i) && exclude.as_raw()[i] <= 127)
        .or_else(|| median(&ring))
        .or_else(|| median(&|i| region.as_raw()[i] > 127))
        .unwrap_or(peniko::Color::from_rgb8(0, 0, 0))
}

/// The outline of `mask` as polylines in bitmap pixels (for drawing the preview).
pub fn outline_points(mask: &GrayImage, opts: &ObjectOptions) -> Vec<Vec<Point>> {
    let mut lines: Vec<Vec<Point>> = Vec::new();
    kurbo::flatten(mask_outline(mask, opts).iter(), 0.5, |el| match el {
        kurbo::PathEl::MoveTo(p) => lines.push(vec![p]),
        kurbo::PathEl::LineTo(p) => {
            if let Some(l) = lines.last_mut() {
                l.push(p);
            }
        }
        kurbo::PathEl::ClosePath => {
            if let Some(l) = lines.last_mut() {
                l.push(l[0]);
            }
        }
        _ => {}
    });
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64) -> ShapeData {
        ShapeData::filled(Rect::new(x, 0.0, x + 100.0, 100.0).to_path(1e-9), peniko::Color::from_rgb8(200, 80, 40))
    }

    /// Inside an open symbol the split draws that symbol's layer with only the shapes asked for. It
    /// used to edit the stage's layers instead and so drew the whole symbol every time: every
    /// object came out as the whole picture (no clicking it, outlines and fillers all wrong).
    #[test]
    fn a_render_inside_a_symbol_shows_only_the_shapes_asked_for() {
        let mut scene = Scene::default();
        let symbol = scene.add_symbol("Art", SymbolKind::Graphic, None);
        assert!(scene.enter_symbol(symbol));
        let layer = scene.layers().iter().next().unwrap().id;
        let left = scene.add_shape_at(layer, 0, rect(0.0)).unwrap();
        scene.add_shape_at(layer, 0, rect(300.0)).unwrap();
        let only: Vec<_> = scene.layers().get(layer).unwrap().frames.objects_at(0).iter().filter(|o| o.id == left).cloned().collect();
        let Ok(img) = render_in(&scene, layer, 0, &only, Rect::new(0.0, 0.0, 400.0, 100.0), (400, 100), true) else {
            return; // no GPU here
        };
        assert!(img.get_pixel(50, 50)[3] > 200, "the shape asked for is drawn");
        assert!(img.get_pixel(350, 50)[3] < 10, "the other one is not");
    }

    /// An object's silhouette loses its pinholes and its specks before it is outlined.
    #[test]
    fn a_silhouette_is_tidied_before_its_outline() {
        let mut m = GrayImage::from_fn(60, 40, |x, y| image::Luma([if (10..50).contains(&x) && (5..35).contains(&y) { 255 } else { 0 }]));
        m.put_pixel(30, 20, image::Luma([0])); // a pinhole
        m.put_pixel(2, 2, image::Luma([255])); // a speck
        let t = tidy_mask(&m, 20, 5);
        assert_eq!(t.get_pixel(30, 20)[0], 255, "the hole is filled");
        assert_eq!(t.get_pixel(2, 2)[0], 0, "the speck is gone");
        assert_eq!(t.get_pixel(12, 6)[0], 255, "the rest stays");
    }

    /// The hole an object leaves takes the colour round it, not another object's.
    #[test]
    fn a_hole_takes_the_colour_round_it() {
        let wall = image::Rgba([90, 100, 80, 255]);
        let mut picture = RgbaImage::from_pixel(80, 60, wall);
        let mut other = GrayImage::new(80, 60);
        for y in 0..60 {
            for x in 50..80 {
                picture.put_pixel(x, y, image::Rgba([200, 30, 30, 255]));
                other.put_pixel(x, y, image::Luma([255]));
            }
        }
        let region = GrayImage::from_fn(80, 60, |x, y| image::Luma([if (30..45).contains(&x) && (20..40).contains(&y) { 255 } else { 0 }]));
        assert_eq!(ring_colour(&picture, &region, &other).to_rgba8().to_u8_array(), [90, 100, 80, 255]);
    }

    /// A split of the middle keyframe of three (F6 copies sharing their objects) takes the
    /// shapes from that keyframe only, and the object's layer shows it over that span alone.
    #[test]
    fn a_split_of_one_keyframe_leaves_the_others_alone() {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;
        let house = scene.add_shape_at(layer, 0, rect(0.0)).unwrap();
        let tree = scene.add_shape_at(layer, 0, rect(300.0)).unwrap();
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(29);
            l.frames.insert_keyframe(10);
            l.frames.insert_keyframe(20);
        });
        let plan = ArtworkPlan {
            layer,
            frame: 12,
            objects: vec![ArtworkObject {
                plate: None,
                behind: Plate::Land,
                label: "House".into(),
                n: 1,
                bbox: [0, 0, 100, 100],
                mask: GrayImage::new(1, 1),
                shape_ids: vec![house],
                cuts: Vec::new(),
                filler: Vec::new(),
                filler_at: Affine::IDENTITY,
                outline: None,
            }],
            notes: Vec::new(),
            picture: RgbaImage::new(1, 1),
            to_world: Affine::IDENTITY,
            record: None,
            removed: Vec::new(),
        };
        assert_eq!(apply(&mut scene, &plan), vec!["House 1".to_string()]);

        let ids = |s: &Scene, l: LayerId, f: u32| -> Vec<ObjectId> { s.layers().get(l).unwrap().frames.objects_at(f).iter().map(|o| o.id).collect() };
        let source = &scene.layers().get(layer).unwrap().frames;
        assert_eq!(source.keyframes().iter().map(|k| k.start).collect::<Vec<_>>(), vec![0, 10, 20], "no keyframe made at the playhead");
        assert_eq!(ids(&scene, layer, 0), vec![house, tree], "the first keyframe keeps the house");
        assert_eq!(ids(&scene, layer, 10), vec![tree], "the split keyframe gave it away");
        assert_eq!(ids(&scene, layer, 20), vec![house, tree], "the last keyframe keeps the house");

        let new = scene.layers().iter().find(|l| l.name == "House 1").unwrap();
        assert_eq!(new.frames.keyframes().iter().map(|k| (k.start, k.objects.len())).collect::<Vec<_>>(), vec![(0, 0), (10, 1), (20, 0)]);
        assert_eq!(new.frames.length(), 30);
        let symbol = new.frames.objects_at(10)[0].instance().unwrap().symbol;
        let inner: Vec<ObjectId> = scene.library().get(symbol).unwrap().layers.iter().next().unwrap().frames.objects_at(0).iter().map(|o| o.id).collect();
        assert_eq!(inner.len(), 1);
        assert_ne!(inner[0], house, "the moved house has an id of its own");
    }
}
