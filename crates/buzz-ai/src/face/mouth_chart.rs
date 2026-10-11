//! Mouth chart import (M1 of `docs/SERIES_EP1_PLAN.md`): a labelled sheet of
//! drawn mouths becomes a character's swappable mouth-symbol frames.
//!
//! # What a chart looks like
//!
//! Raju's chart is the model: a warm off-white page, the mouths in rows, a
//! text label under each. The reader finds every mouth, hands each one its
//! label **by reading order against a list the caller gives** (no OCR: the
//! user knows what their chart says), traces it with the same tracer the
//! character pipeline uses, and returns every mouth at one scale (the closed
//! mouth is 100 units wide), each centred on the origin.
//!
//! # How a mouth is told from a label
//!
//! By colour first. Text is neutral ink; a mouth is lips, tongue and a dark
//! brown line, so a connected piece of ink with a real share of *coloured*
//! (non-grey) pixels is mouth, and one without is lettering. That is robust
//! where size and position are not: a label can sit a few pixels under a wide
//! laugh and still never merge into it. The pieces of one mouth (a lip, its
//! highlight, a stray whisker) are joined by a small gap, mouths are far
//! apart, so the groups are the mouths. Teeth are near the page's colour, so
//! each mouth's outline is **hole-filled** to keep them in.
//!
//! The assumption, stated plainly: the mouths are coloured and the labels are
//! grey or black. A chart of pure line art would need another rule.
//!
//! # What the frames are
//!
//! [`frame_plan`] lays the mouths onto the existing mouth symbol: frames
//! `0..=9` are the visemes in lip-sync order (so `write_track` keeps working),
//! every other mouth is appended as an extra **named** frame the director can
//! key by hand (`Small Smile`, `Frown`, `Open Laugh`...).

use std::path::Path;
use std::sync::Arc;

use kurbo::{Affine, Point, Rect, Shape as _};
use peniko::Color;

use buzz_audio::Viseme;
use buzz_scene::{Layer, LayerId, LayerKind, LayerStack, Object, ObjectId, ObjectKind, Scene, ShapeData, Symbol, SymbolId};

/// Raju's chart, in reading order. The default for `ai_mouth_chart`.
pub const RAJU_LABELS: [&str; 19] = [
    "Neutral Closed",
    "Small Smile",
    "Broad Smile",
    "Frown",
    "A",
    "E",
    "I",
    "O",
    "U",
    "M/B/P",
    "F/V",
    "L",
    "TH",
    "CH/J",
    "S/Z",
    "Open Laugh",
    "Angry Tight Mouth",
    "Confused Half-Open",
    "Skeptical Side-Mouth",
];

/// The closed mouth's width in the chart's own units. Every mouth is returned
/// at one common scale in which the closed mouth is this wide, each one
/// centred on the origin by its own box; the caller scales that to the face
/// it is for.
pub const UNIT_WIDTH: f64 = 100.0;

/// Colour distance (RGB, 0..441) from the page colour past which a pixel is ink.
const INK_DISTANCE: f64 = 40.0;
/// `max(r,g,b) - min(r,g,b)` from which a pixel counts as coloured, not grey.
const COLOUR_CHROMA: i32 = 30;
/// A piece of ink with this share of coloured pixels is mouth, not lettering.
const MOUTH_WARM_SHARE: f64 = 0.30;
/// Colours in the chart's shared palette: skin, lip shade, interior, tongue,
/// teeth, the dark rim and a couple of shadows.
const PALETTE_COLOURS: usize = 8;

/// One mouth cut from a chart and traced.
#[derive(Debug, Clone)]
pub struct ChartMouth {
    /// The label it was given, from the caller's list.
    pub label: String,
    /// The traced artwork, in chart units: centred on the origin, at the scale
    /// where the closed mouth is [`UNIT_WIDTH`] wide.
    pub shapes: Vec<ShapeData>,
    /// The artwork's box in the same units (stroke width not included).
    pub bounds: Rect,
    /// Where it was cut from, in the chart image's own pixels.
    pub source_box: Rect,
}

/// A whole chart: every labelled mouth, in the order the labels were given.
#[derive(Debug, Clone)]
pub struct MouthChart {
    pub mouths: Vec<ChartMouth>,
    /// Which mouth is the closed one: the scale unit.
    pub rest_index: usize,
    /// Chart pixels per chart unit (the closed mouth's pixel width / 100).
    pub pixels_per_unit: f64,
    /// The stroke width the tracer used, in chart units.
    pub line_width: f64,
    /// Things the importer worked around, for the report.
    pub notes: Vec<String>,
}

impl MouthChart {
    pub fn mouth(&self, label: &str) -> Option<&ChartMouth> {
        let want = norm(label);
        self.mouths.iter().find(|m| norm(&m.label) == want)
    }
}

/// Lower-case letters and digits only, so `M/B/P`, `mbp` and `M B P` agree.
fn norm(label: &str) -> String {
    label.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Read, slice and trace a chart file. See [`read_chart`].
pub fn read_chart_file(path: &Path, labels: &[&str]) -> Result<MouthChart, String> {
    let image = image::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    read_chart(&image.to_rgba8(), labels)
}

/// Find the mouths in a chart image, label them in reading order, trace each
/// and normalise them to one scale, each centred on the origin.
///
/// `labels` are in reading order: rows top to bottom, each row left to right.
/// The number of mouths found must equal `labels.len()`; when it does not the
/// error says how many were found in each row, so a missing or extra mouth is
/// easy to place.
pub fn read_chart(img: &image::RgbaImage, labels: &[&str]) -> Result<MouthChart, String> {
    if labels.is_empty() {
        return Err("no labels given".into());
    }
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 16 || h < 16 {
        return Err("the chart image is too small".into());
    }
    let rgb = composite_on_white(img);
    let bg = page_colour(&rgb, w, h);
    let ink: Vec<bool> = rgb.iter().map(|p| colour_distance(*p, bg) > INK_DISTANCE).collect();

    let (piece_map, piece_count) = label_components(&ink, w, h);
    let pieces = piece_stats(&piece_map, piece_count, &rgb, w);
    let min_px = ((w * h) as f64 * 1e-5).max(12.0) as usize;
    let mouth_like: Vec<bool> = pieces
        .iter()
        .map(|p| p.area >= min_px && p.warm as f64 / p.area as f64 >= MOUTH_WARM_SHARE)
        .collect();
    if !mouth_like.iter().any(|&m| m) {
        return Err("no coloured mouth artwork found in the chart (the reader expects coloured mouths and grey/black labels)".into());
    }

    // The pieces of one mouth are a few pixels apart; two mouths are not.
    let gap = ((w.max(h) as f64 * 0.008).round() as usize).max(3);
    let mouth_ink: Vec<bool> = piece_map.iter().map(|&p| p != 0 && mouth_like[p as usize - 1]).collect();
    let (group_map, group_count) = label_components(&dilate(&mouth_ink, w, h, gap), w, h);
    let mut group_of_piece = vec![0u32; pieces.len()];
    for (i, &p) in piece_map.iter().enumerate() {
        if mouth_ink[i] && group_of_piece[p as usize - 1] == 0 {
            group_of_piece[p as usize - 1] = group_map[i];
        }
    }
    let mut blobs: Vec<Blob> = (0..group_count).map(|_| Blob::empty()).collect();
    for (pi, piece) in pieces.iter().enumerate() {
        let g = group_of_piece[pi];
        if g != 0 {
            blobs[g as usize - 1].add(piece, pi);
        }
    }
    blobs.retain(|b| b.area > 0);

    // Specks: far smaller than the typical mouth.
    let mut notes = Vec::new();
    let median_area = median(&mut blobs.iter().map(|b| b.area as f64).collect::<Vec<_>>());
    let before = blobs.len();
    blobs.retain(|b| b.area as f64 >= median_area * 0.03);
    if blobs.len() != before {
        notes.push(format!("ignored {} speck(s) of colour that are too small to be a mouth", before - blobs.len()));
    }

    // Lettering inside a mouth's own box (a dark fragment of its outline that
    // the gap rule left out) belongs to the mouth.
    for (pi, piece) in pieces.iter().enumerate() {
        if mouth_like[pi] {
            continue;
        }
        if let Some(b) = blobs.iter_mut().find(|b| b.contains_box(piece)) {
            b.add(piece, pi);
        }
    }

    let row_sizes = group_into_rows(&mut blobs);
    if blobs.len() != labels.len() {
        return Err(format!(
            "found {} mouths ({} per row, top to bottom) but {} labels were given; check the label list matches the chart",
            blobs.len(),
            row_sizes.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("+"),
            labels.len()
        ));
    }
    // `group_into_rows` leaves `blobs` in reading order.

    // Which mouth is the closed one.
    let rest_index = labels
        .iter()
        .position(|l| matches!(norm(l).as_str(), "neutralclosed" | "closed" | "rest" | "neutral"))
        .unwrap_or(0);
    let closed_px = blobs[rest_index].rect().width().max(1.0);
    let pixels_per_unit = closed_px / UNIT_WIDTH;
    let k = 1.0 / pixels_per_unit;

    // One palette for the whole chart, so skin, tongue and teeth are the same
    // colours in every mouth.
    let masks: Vec<Vec<bool>> = blobs.iter().map(|b| b.mask(&piece_map, w)).collect();
    let mut all = vec![0u8; w * h * 4];
    for (b, m) in blobs.iter().zip(&masks) {
        let r = b.rect_px();
        let cw = r.2 - r.0;
        for y in r.1..r.3 {
            for x in r.0..r.2 {
                if m[(y - r.1) * cw + (x - r.0)] {
                    let o = (y * w + x) * 4;
                    all[o..o + 3].copy_from_slice(&rgb[y * w + x]);
                    all[o + 3] = 255;
                }
            }
        }
    }
    let palette = buzz_scene::quantise(&all, w, h, PALETTE_COLOURS);
    if palette.is_empty() {
        return Err("the chart's mouths have no colour to trace".into());
    }
    let line_colour = palette
        .iter()
        .min_by_key(|c| c[0] as u32 + c[1] as u32 + c[2] as u32)
        .map(|c| Color::from_rgb8(c[0], c[1], c[2]))
        .unwrap_or(Color::from_rgb8(40, 20, 15));
    let line_px = (w.max(h) as f64 * 0.002).clamp(1.5, 6.0);

    let mut mouths = Vec::with_capacity(blobs.len());
    for ((b, m), label) in blobs.iter().zip(&masks).zip(labels) {
        let (x0, y0, x1, y1) = b.rect_px();
        let (cw, ch) = (x1 - x0, y1 - y0);
        let mut crop = vec![0u8; cw * ch * 4];
        for y in 0..ch {
            for x in 0..cw {
                if m[y * cw + x] {
                    let o = (y * cw + x) * 4;
                    crop[o..o + 3].copy_from_slice(&rgb[(y0 + y) * w + (x0 + x)]);
                    crop[o + 3] = 255;
                }
            }
        }
        let traced = trace_mouth(&crop, cw, ch, &palette, line_colour, line_px);
        // Each mouth is centred on the origin by its own box, so swapping one
        // frame for another keeps the mouth on the same spot of the face.
        let own = b.rect().center();
        let to_units = Affine::scale(k) * Affine::translate((x0 as f64 - own.x, y0 as f64 - own.y));
        let mut shapes = Vec::with_capacity(traced.len());
        for mut shape in traced {
            if let Some(fill) = &mut shape.fill {
                let c = fill.color().to_rgba8();
                fill.paint = buzz_scene::object::Paint::Solid(snap([c.r, c.g, c.b], &palette));
            }
            shape.path = to_units * shape.path;
            if let Some(stroke) = &mut shape.stroke {
                stroke.width *= k;
            }
            shapes.push(shape);
        }
        if shapes.is_empty() {
            return Err(format!("tracing the '{label}' mouth produced nothing"));
        }
        let mut bounds: Option<Rect> = None;
        for s in &shapes {
            let bb = s.path.bounding_box();
            bounds = Some(bounds.map_or(bb, |all| all.union(bb)));
        }
        mouths.push(ChartMouth {
            label: (*label).to_string(),
            shapes,
            bounds: bounds.unwrap_or(Rect::ZERO),
            source_box: b.rect(),
        });
    }

    Ok(MouthChart { mouths, rest_index, pixels_per_unit, line_width: line_px * k, notes })
}

// -- frames -----------------------------------------------------------------------

/// One frame of the mouth symbol and which chart mouth fills it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedFrame {
    /// The symbol frame, 0-based.
    pub index: u32,
    /// The frame's name: the viseme's label for frames `0..=9`, the chart
    /// label for the extra frames.
    pub name: String,
    /// The chart mouth that fills it (index into [`MouthChart::mouths`]).
    pub mouth: usize,
    /// The viseme it stands for; `None` on an extra frame.
    pub viseme: Option<Viseme>,
}

/// Labels that stand for each viseme, best first. WQ has no mouth of its own
/// on a chart, so it takes the rounded U; Etc takes the closest-to-neutral
/// teeth mouth.
fn viseme_sources(viseme: Viseme) -> &'static [&'static str] {
    match viseme {
        Viseme::Rest => &["neutralclosed", "closed", "neutral", "rest"],
        Viseme::Ai => &["a", "ai", "ah"],
        Viseme::E => &["e", "eh"],
        Viseme::O => &["o", "oh"],
        Viseme::U => &["u", "oo"],
        Viseme::L => &["l"],
        Viseme::WQ => &["wq", "w", "q", "u", "oo"],
        Viseme::MBP => &["mbp", "mb", "bmp", "m", "b", "p"],
        Viseme::FV => &["fv", "f", "v"],
        Viseme::Etc => &["etc", "sz", "chj", "th", "i"],
    }
}

/// Which chart mouth goes on which frame: the ten visemes in lip-sync order,
/// then every mouth not used by one of them as an extra named frame, in chart
/// order. A viseme the chart has no label for falls back to the closed mouth
/// (said in the chart's notes by [`describe_plan`]).
pub fn frame_plan(chart: &MouthChart) -> Vec<PlannedFrame> {
    let mut plan: Vec<PlannedFrame> = Vec::new();
    let mut used = vec![false; chart.mouths.len()];
    let mut order = [Viseme::Rest; 10];
    for v in [
        Viseme::Rest,
        Viseme::Ai,
        Viseme::E,
        Viseme::O,
        Viseme::U,
        Viseme::L,
        Viseme::WQ,
        Viseme::MBP,
        Viseme::FV,
        Viseme::Etc,
    ] {
        order[v.frame() as usize] = v;
    }
    for v in order {
        let found = viseme_sources(v)
            .iter()
            .find_map(|want| chart.mouths.iter().position(|m| norm(&m.label) == *want));
        let mouth = found.unwrap_or(chart.rest_index);
        used[mouth] = true;
        plan.push(PlannedFrame { index: v.frame(), name: v.label().to_string(), mouth, viseme: Some(v) });
    }
    let mut next = Viseme::COUNT;
    for (i, m) in chart.mouths.iter().enumerate() {
        if !used[i] {
            plan.push(PlannedFrame { index: next, name: m.label.clone(), mouth: i, viseme: None });
            next += 1;
        }
    }
    plan
}

/// The plan in words, with a note for any viseme that had to fall back.
pub fn describe_plan(chart: &MouthChart) -> Vec<String> {
    frame_plan(chart)
        .iter()
        .map(|f| match f.viseme {
            Some(v) => {
                let fell_back = !viseme_sources(v).iter().any(|w| norm(&chart.mouths[f.mouth].label) == *w);
                let note = if fell_back { " (no label for it on the chart: the closed mouth)" } else { "" };
                format!("{} {} = {}{}", f.index, f.name, chart.mouths[f.mouth].label, note)
            }
            None => format!("{} {}", f.index, f.name),
        })
        .collect()
}

/// The symbol layer holding every frame of the plan.
///
/// `scale` turns chart units into the face's units (`rest width / UNIT_WIDTH`);
/// `offset` is where every mouth's centre goes; `line_width` forces every
/// stroke to one width in face units (the character's own line weight), or
/// `None` scales the chart's own. Every frame is a keyframe with its name as
/// the frame label, and its shapes are named for the chart mouth.
pub fn build_layer(
    chart: &MouthChart,
    layer_id: LayerId,
    layer_name: &str,
    scale: f64,
    offset: Point,
    line_width: Option<f64>,
    next_id: &mut dyn FnMut() -> ObjectId,
) -> Layer {
    let mut layer = Layer::new(layer_id, layer_name, LayerKind::Normal);
    let place = Affine::translate(offset.to_vec2()) * Affine::scale(scale);
    for frame in frame_plan(chart) {
        layer.frames.insert_keyframe(frame.index);
        let mouth = &chart.mouths[frame.mouth];
        let objects: Vec<Arc<Object>> = mouth
            .shapes
            .iter()
            .map(|shape| {
                let mut shape = shape.clone();
                shape.path = place * shape.path;
                if let Some(stroke) = &mut shape.stroke {
                    stroke.width = line_width.unwrap_or(stroke.width * scale);
                }
                let mut object = Object::shape(next_id(), shape);
                object.name = Some(mouth.label.clone());
                Arc::new(object)
            })
            .collect();
        layer.frames.set_objects(frame.index, objects);
        layer.frames.set_label(frame.index, Some(frame.name.clone()));
    }
    layer
}

/// A new mouth symbol made from a chart, at `scale`, every mouth centred on
/// the origin (an instance is placed at the mouth's centre).
pub fn build_mouth_symbol(
    sym_id: SymbolId,
    name: &str,
    chart: &MouthChart,
    scale: f64,
    line_width: Option<f64>,
    next_id: &mut dyn FnMut() -> ObjectId,
) -> Symbol {
    let mut sym = Symbol::new(sym_id, name.to_string(), buzz_scene::SymbolKind::Graphic);
    let layer = build_layer(chart, LayerId(1), "Mouth Shapes", scale, Point::ORIGIN, line_width, next_id);
    sym.layers.push_front(layer);
    sym
}

/// What replacing a character's mouth frames did.
#[derive(Debug, Clone)]
pub struct AppliedChart {
    pub symbol: SymbolId,
    /// `(frame, name)` for every frame now in the symbol.
    pub frames: Vec<(u32, String)>,
    /// Chart units to face units.
    pub scale: f64,
    /// The closed mouth's width in face units.
    pub rest_width: f64,
    /// The stroke width every line was given, when the symbol had one to match.
    pub line_width: Option<f64>,
    /// Layers the symbol had before (all replaced by the one chart layer).
    pub layers_replaced: usize,
}

/// Replace the frames of `symbol` (a character's mouth symbol) with the chart's.
///
/// The closed mouth is sized to the symbol's current Rest frame (frame 0) and
/// every mouth is centred where that frame is, so the new mouths sit on the face exactly where the
/// old ones did; every line takes the stroke width the symbol already used,
/// if it used strokes. The symbol keeps its id, so every instance of it, and
/// every lip-sync keyframe that shows one of its frames, carries on working.
pub fn replace_mouth_frames(
    scene: &mut Scene,
    symbol: SymbolId,
    chart: &MouthChart,
) -> Result<AppliedChart, String> {
    let (rest_box, line_width, first_layer, layers_before) = {
        let Some(sym) = scene.library().get(symbol) else {
            return Err(format!("no symbol {}", symbol.0));
        };
        (
            sym.layers.iter().filter_map(|l| rest_frame_bounds(l)).reduce(|a, b| a.union(b)),
            stroke_width_of(sym),
            sym.layers.iter().next().map(|l| (l.id, l.name.clone())),
            sym.layers.len(),
        )
    };
    let rest_box = rest_box
        .filter(|r| r.width() > 0.5)
        .unwrap_or_else(|| Rect::from_center_size((0.0, 0.0), (28.0, 8.0)));
    let (layer_id, layer_name) = first_layer.unwrap_or((LayerId(1), "Mouth Shapes".to_string()));
    let scale = rest_box.width() / UNIT_WIDTH;

    let layer = build_layer(chart, layer_id, &layer_name, scale, rest_box.center(), line_width, &mut || scene.next_object_id());
    let frames: Vec<(u32, String)> = frame_plan(chart).into_iter().map(|f| (f.index, f.name)).collect();
    scene.library_mut().update(symbol, |s| {
        let mut stack = LayerStack::new();
        stack.push_front(layer);
        s.layers = stack;
    });
    Ok(AppliedChart { symbol, frames, scale, rest_width: rest_box.width(), line_width, layers_replaced: layers_before })
}

/// The box of what a layer shows on frame 0.
fn rest_frame_bounds(layer: &Layer) -> Option<Rect> {
    layer
        .frames
        .objects_at(0)
        .iter()
        .map(|o| o.bounds())
        .filter(|b| *b != Rect::ZERO)
        .reduce(|a, b| a.union(b))
}

/// The stroke width the symbol's artwork already uses (the median of its
/// stroked shapes), so new mouths keep the character's line weight.
fn stroke_width_of(sym: &Symbol) -> Option<f64> {
    let mut widths: Vec<f64> = Vec::new();
    for layer in sym.layers.iter() {
        for object in layer.frames.all_objects() {
            if let ObjectKind::Shape(shape) = &object.kind {
                if let Some(stroke) = &shape.stroke {
                    if stroke.width > 0.0 && !stroke.hairline {
                        widths.push(stroke.width);
                    }
                }
            }
        }
    }
    if widths.is_empty() {
        None
    } else {
        Some(median(&mut widths))
    }
}

// -- segmentation -----------------------------------------------------------------

fn composite_on_white(img: &image::RgbaImage) -> Vec<[u8; 3]> {
    img.pixels()
        .map(|p| {
            let a = p[3] as u32;
            let over = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
            [over(p[0]), over(p[1]), over(p[2])]
        })
        .collect()
}

/// The page colour: the median of the image's border ring.
fn page_colour(rgb: &[[u8; 3]], w: usize, h: usize) -> [u8; 3] {
    let ring = 4.min(w / 4).min(h / 4).max(1);
    let mut chans: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for y in 0..h {
        for x in 0..w {
            if x < ring || y < ring || x >= w - ring || y >= h - ring {
                let p = rgb[y * w + x];
                for c in 0..3 {
                    chans[c].push(p[c]);
                }
            }
        }
    }
    let mid = |v: &mut Vec<u8>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    [mid(&mut chans[0]), mid(&mut chans[1]), mid(&mut chans[2])]
}

fn colour_distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    let d = |i: usize| (a[i] as f64 - b[i] as f64).powi(2);
    (d(0) + d(1) + d(2)).sqrt()
}

/// One 8-connected piece of ink.
struct Piece {
    area: usize,
    warm: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

/// 8-connected labels (1-based, 0 = not in the mask) and how many there are.
fn label_components(mask: &[bool], w: usize, h: usize) -> (Vec<u32>, usize) {
    let mut map = vec![0u32; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let mut count = 0u32;
    for start in 0..w * h {
        if !mask[start] || map[start] != 0 {
            continue;
        }
        count += 1;
        map[start] = count;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if mask[n] && map[n] == 0 {
                        map[n] = count;
                        stack.push(n);
                    }
                }
            }
        }
    }
    (map, count as usize)
}

/// Area, coloured pixels and box of every labelled piece.
fn piece_stats(map: &[u32], count: usize, rgb: &[[u8; 3]], w: usize) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = (0..count)
        .map(|_| Piece { area: 0, warm: 0, x0: usize::MAX, y0: usize::MAX, x1: 0, y1: 0 })
        .collect();
    for (i, &label) in map.iter().enumerate() {
        if label == 0 {
            continue;
        }
        let (x, y) = (i % w, i / w);
        let piece = &mut pieces[label as usize - 1];
        piece.area += 1;
        let p = rgb[i];
        if p.iter().copied().max().unwrap() as i32 - p.iter().copied().min().unwrap() as i32 >= COLOUR_CHROMA {
            piece.warm += 1;
        }
        piece.x0 = piece.x0.min(x);
        piece.y0 = piece.y0.min(y);
        piece.x1 = piece.x1.max(x + 1);
        piece.y1 = piece.y1.max(y + 1);
    }
    pieces
}

fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut across = vec![false; w * h];
    let mut prefix = vec![0u32; w.max(h) + 1];
    for y in 0..h {
        for x in 0..w {
            prefix[x + 1] = prefix[x] + mask[y * w + x] as u32;
        }
        for x in 0..w {
            across[y * w + x] = prefix[(x + r + 1).min(w)] - prefix[x.saturating_sub(r)] > 0;
        }
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        for y in 0..h {
            prefix[y + 1] = prefix[y] + across[y * w + x] as u32;
        }
        for y in 0..h {
            out[y * w + x] = prefix[(y + r + 1).min(h)] - prefix[y.saturating_sub(r)] > 0;
        }
    }
    out
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values[values.len() / 2]
}

/// The snapped colour: the palette entry nearest in RGB.
fn snap(c: [u8; 3], palette: &[[u8; 3]]) -> Color {
    let best = palette
        .iter()
        .min_by(|a, b| colour_distance(c, **a).partial_cmp(&colour_distance(c, **b)).unwrap_or(std::cmp::Ordering::Equal))
        .copied()
        .unwrap_or(c);
    Color::from_rgb8(best[0], best[1], best[2])
}

/// One mouth: the pieces of ink that make it up and their joint box.
struct Blob {
    pieces: Vec<usize>,
    area: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Blob {
    fn empty() -> Self {
        Blob { pieces: Vec::new(), area: 0, x0: usize::MAX, y0: usize::MAX, x1: 0, y1: 0 }
    }

    fn add(&mut self, piece: &Piece, index: usize) {
        self.pieces.push(index);
        self.area += piece.area;
        self.x0 = self.x0.min(piece.x0);
        self.y0 = self.y0.min(piece.y0);
        self.x1 = self.x1.max(piece.x1);
        self.y1 = self.y1.max(piece.y1);
    }

    fn contains_box(&self, piece: &Piece) -> bool {
        self.area > 0 && piece.x0 >= self.x0 && piece.y0 >= self.y0 && piece.x1 <= self.x1 && piece.y1 <= self.y1
    }

    fn rect(&self) -> Rect {
        Rect::new(self.x0 as f64, self.y0 as f64, self.x1 as f64, self.y1 as f64)
    }

    /// The crop box: the mouth's box with a little air, in pixels.
    fn rect_px(&self) -> (usize, usize, usize, usize) {
        (self.x0.saturating_sub(PAD), self.y0.saturating_sub(PAD), self.x1 + PAD, self.y1 + PAD)
    }

    /// The mouth's mask over its crop box, holes (teeth) filled.
    fn mask(&self, piece_map: &[u32], w: usize) -> Vec<bool> {
        let (x0, y0, x1, y1) = self.rect_px();
        let h_full = piece_map.len() / w;
        let (x1, y1) = (x1.min(w), y1.min(h_full));
        let (cw, ch) = (x1 - x0, y1 - y0);
        let mine: std::collections::HashSet<u32> = self.pieces.iter().map(|&p| p as u32 + 1).collect();
        let mut mask = vec![false; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                let p = piece_map[(y0 + y) * w + (x0 + x)];
                mask[y * cw + x] = p != 0 && mine.contains(&p);
            }
        }
        fill_holes(&mut mask, cw, ch);
        mask
    }
}

/// Air around a mouth's box in its crop, in pixels.
const PAD: usize = 3;

/// Fill every background pixel that cannot reach the crop's edge without
/// crossing the mask (4-connected, so a diagonal gap in an outline does not
/// let the outside in): the teeth and the inside of an open mouth.
fn fill_holes(mask: &mut [bool], w: usize, h: usize) {
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        for y in [0, h - 1] {
            let i = y * w + x;
            if !mask[i] && !outside[i] {
                outside[i] = true;
                stack.push(i);
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            let i = y * w + x;
            if !mask[i] && !outside[i] {
                outside[i] = true;
                stack.push(i);
            }
        }
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut visit = |n: usize| {
            if !mask[n] && !outside[n] {
                outside[n] = true;
                stack.push(n);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < w {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - w);
        }
        if y + 1 < h {
            visit(i + w);
        }
    }
    for i in 0..w * h {
        if !outside[i] {
            mask[i] = true;
        }
    }
}

/// Order the mouths for reading (rows top to bottom, each left to right) and
/// return the row lengths. Rows are where the mouths' vertical centres jump
/// by more than half a typical mouth's height.
fn group_into_rows(blobs: &mut Vec<Blob>) -> Vec<usize> {
    if blobs.is_empty() {
        return Vec::new();
    }
    let mut heights: Vec<f64> = blobs.iter().map(|b| (b.y1 - b.y0) as f64).collect();
    let threshold = median(&mut heights) * 0.5;
    let centre_y = |b: &Blob| (b.y0 + b.y1) as f64 / 2.0;
    let centre_x = |b: &Blob| (b.x0 + b.x1) as f64 / 2.0;
    blobs.sort_by(|a, b| centre_y(a).partial_cmp(&centre_y(b)).unwrap_or(std::cmp::Ordering::Equal));
    let mut rows: Vec<Vec<Blob>> = Vec::new();
    for blob in std::mem::take(blobs) {
        match rows.last_mut() {
            Some(row) if (centre_y(&blob) - centre_y(row.last().unwrap())).abs() <= threshold => row.push(blob),
            _ => rows.push(vec![blob]),
        }
    }
    let mut sizes = Vec::new();
    for mut row in rows {
        row.sort_by(|a, b| centre_x(a).partial_cmp(&centre_x(b)).unwrap_or(std::cmp::Ordering::Equal));
        sizes.push(row.len());
        blobs.extend(row);
    }
    sizes
}

// -- tracing ----------------------------------------------------------------------

/// Trace one mouth crop (RGBA, alpha = the mouth) with the shared tracer, in
/// the crop's own pixels.
///
/// The crop is **posterised here first**, to the chart's palette, and handed
/// to the tracer already flat. The tracer's contour walk
/// (`bucket::trace_contours`) loses an edge wherever two pixels of one region
/// touch only at a corner, and the loop it was walking is then closed by a
/// straight chord: a wedge of the wrong colour cut across the mouth. Soft,
/// shaded artwork like a chart's is full of such corners. So the class map is
/// built at the tracer's working resolution, here, and every corner contact is
/// thickened away before the tracer sees it; its own quantiser then finds the
/// palette colours exactly and the regions it traces are the ones made here.
fn trace_mouth(
    crop: &[u8],
    cw: usize,
    ch: usize,
    palette: &[[u8; 3]],
    line_colour: Color,
    line_px: f64,
) -> Vec<ShapeData> {
    const NONE: u8 = u8::MAX;
    let n = palette.len();
    let palette_lab: Vec<[f64; 3]> = palette.iter().map(|&c| buzz_scene::trace::srgb_to_lab(c)).collect();

    // 1. Each opaque pixel's palette class, with anti-aliasing blends joined to
    //    the nearer of the two colours they lie between.
    let mut class = vec![NONE; cw * ch];
    let mut lab = vec![[0.0f64; 3]; cw * ch];
    for i in 0..cw * ch {
        if crop[i * 4 + 3] >= 128 {
            lab[i] = buzz_scene::trace::srgb_to_lab([crop[i * 4], crop[i * 4 + 1], crop[i * 4 + 2]]);
            class[i] = nearest(&palette_lab, lab[i]);
        }
    }
    reassign_blends(&mut class, &lab, &palette_lab, cw, ch, NONE);

    // 2. Up to the tracer's working size (at least 1024 on the long edge, as
    //    the tracer itself would) by smoothing each class's own coverage and
    //    taking the strongest, so class edges are smooth curves and no band of
    //    in-between colour appears along them.
    let long = cw.max(ch);
    let factor = 1024usize.div_ceil(long).max(1);
    let (uw, uh) = (cw * factor, ch * factor);
    let mut strongest = vec![0u8; uw * uh];
    let mut up = vec![NONE; uw * uh];
    for c in 0..=n {
        let want = if c == n { NONE } else { c as u8 };
        let coverage = image::GrayImage::from_fn(cw as u32, ch as u32, |x, y| {
            image::Luma([if class[y as usize * cw + x as usize] == want { 255 } else { 0 }])
        });
        let big = if factor > 1 {
            image::imageops::resize(&coverage, uw as u32, uh as u32, image::imageops::FilterType::CatmullRom)
        } else {
            coverage
        };
        for (i, p) in big.pixels().enumerate() {
            if p[0] > strongest[i] {
                strongest[i] = p[0];
                up[i] = want;
            }
        }
    }

    // 3. No corner-only contacts.
    thicken_corner_contacts(&mut up, uw, uh, NONE);
    debug_assert!(!has_corner_contact(&up, uw, uh, NONE), "a corner contact survived thickening");

    // 4. Exact palette colours in, shapes out.
    let mut rgba = vec![0u8; uw * uh * 4];
    let mut used = vec![false; n];
    for (i, &c) in up.iter().enumerate() {
        if c != NONE {
            rgba[i * 4..i * 4 + 3].copy_from_slice(&palette[c as usize]);
            rgba[i * 4 + 3] = 255;
            used[c as usize] = true;
        }
    }
    let f = factor as f64;
    let options = buzz_scene::TraceOptions {
        mode: buzz_scene::TraceMode::ColourLines,
        line_style: buzz_scene::LineStyle::Stroke,
        uniform_width: Some(line_px * f),
        line_smooth: 0.6,
        fill_enclosed: true,
        colours: used.iter().filter(|u| **u).count().max(2),
        line_colour: Some(line_colour),
        // Specks and detail are in the picture's own pixels; the tracer would
        // have scaled them by its own upscale, which is done here instead.
        speckle: (24.0 * f * f) as usize,
        detail: 1.5 * f,
        min_line_length: 8.0 * f,
        ..Default::default()
    };
    let report = buzz_scene::trace(uw as u32, uh as u32, &rgba, &options);
    let down = Affine::scale(1.0 / f);
    report
        .shapes
        .into_iter()
        .map(|mut shape| {
            shape.path = down * shape.path;
            if let Some(stroke) = &mut shape.stroke {
                stroke.width /= f;
            }
            shape
        })
        .collect()
}

fn nearest(palette_lab: &[[f64; 3]], c: [f64; 3]) -> u8 {
    palette_lab
        .iter()
        .enumerate()
        .min_by(|a, b| buzz_scene::trace::delta_e(c, *a.1).total_cmp(&buzz_scene::trace::delta_e(c, *b.1)))
        .map(|(i, _)| i as u8)
        .unwrap_or(0)
}

/// A pixel that lies on the straight blend between two colours present in its
/// own 3x3 neighbourhood is anti-aliasing, not a colour of its own: it joins
/// the nearer of the two. (The tracer does the same on its own grid.)
fn reassign_blends(class: &mut [u8], lab: &[[f64; 3]], palette_lab: &[[f64; 3]], w: usize, h: usize, none: u8) {
    let original = class.to_vec();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if original[i] == none {
                continue;
            }
            let mut around: Vec<u8> = Vec::with_capacity(9);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                        continue;
                    }
                    let c = original[ny as usize * w + nx as usize];
                    if c != none && !around.contains(&c) {
                        around.push(c);
                    }
                }
            }
            if around.len() < 2 {
                continue;
            }
            let mut pick: Option<(f64, u8, u8)> = None;
            for a in 0..around.len() {
                for b in (a + 1)..around.len() {
                    let d = distance_to_blend(lab[i], palette_lab[around[a] as usize], palette_lab[around[b] as usize]);
                    if d < 6.0 && pick.is_none_or(|(best, _, _)| d < best) {
                        pick = Some((d, around[a], around[b]));
                    }
                }
            }
            if let Some((_, a, b)) = pick {
                let da = buzz_scene::trace::delta_e(lab[i], palette_lab[a as usize]);
                let db = buzz_scene::trace::delta_e(lab[i], palette_lab[b as usize]);
                class[i] = if da <= db { a } else { b };
            }
        }
    }
}

fn distance_to_blend(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    if len2 < 1e-9 {
        return buzz_scene::trace::delta_e(p, a);
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2]) / len2).clamp(0.0, 1.0);
    buzz_scene::trace::delta_e(p, [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t])
}

/// Is there any 2x2 block in which one class occupies a diagonal pair and the
/// other two cells are something else?
fn has_corner_contact(class: &[u8], w: usize, h: usize, none: u8) -> bool {
    (0..h.saturating_sub(1)).any(|y| {
        (0..w.saturating_sub(1)).any(|x| {
            let (a, b, c, d) = (class[y * w + x], class[y * w + x + 1], class[(y + 1) * w + x], class[(y + 1) * w + x + 1]);
            (a == d && a != none && b != a && c != a) || (b == c && b != none && a != b && d != b)
        })
    })
}

/// Remove every place two cells of one class meet only at a corner (the other
/// two cells of that 2x2 block being something else) by giving the class one
/// more cell there, so each class's pixels are joined edge to edge.
fn thicken_corner_contacts(class: &mut [u8], w: usize, h: usize, none: u8) {
    for _ in 0..64 {
        let mut changed = false;
        for y in 0..h.saturating_sub(1) {
            for x in 0..w.saturating_sub(1) {
                let (a, b, c, d) = (y * w + x, y * w + x + 1, (y + 1) * w + x, (y + 1) * w + x + 1);
                let (ca, cb, cc, cd) = (class[a], class[b], class[c], class[d]);
                if ca == cd && ca != none && cb != ca && cc != ca {
                    // Prefer to take the cell that is background; otherwise the second one.
                    let take = if cb == none { b } else { c };
                    class[take] = ca;
                    changed = true;
                } else if cb == cc && cb != none && ca != cb && cd != cb {
                    let take = if ca == none { a } else { d };
                    class[take] = cb;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_contacts_are_thickened_away() {
        const NONE: u8 = u8::MAX;
        // Two classes meeting like a chessboard along a diagonal, in a field of a third.
        let (w, h) = (8usize, 8usize);
        let mut class = vec![2u8; w * h];
        for i in 0..6 {
            class[(i + 1) * w + i + 1] = 0;
            class[(i + 1) * w + i + 2] = 1;
        }
        class[3 * w + 3] = 0;
        class[3 * w + 4] = NONE;
        class[4 * w + 3] = NONE;
        assert!(has_corner_contact(&class, w, h, NONE), "the fixture must start with a corner contact");
        thicken_corner_contacts(&mut class, w, h, NONE);
        assert!(!has_corner_contact(&class, w, h, NONE));
    }

    /// The longest closing segment of any subpath of a filled shape.
    fn worst_closing_gap(path: &kurbo::BezPath) -> f64 {
        use kurbo::PathEl;
        let (mut start, mut last, mut worst) = (Point::ORIGIN, Point::ORIGIN, 0.0f64);
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    start = p;
                    last = p;
                }
                PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => last = p,
                PathEl::ClosePath => worst = worst.max(last.distance(start)),
            }
        }
        worst
    }

    /// A mouth crossed by a thin diagonal line, a staircase of corner contacts,
    /// traces without any outline being closed by a long chord.
    #[test]
    fn a_thin_diagonal_line_traces_without_a_chord() {
        let (w, h) = (90usize, 50usize);
        let skin = [222u8, 148, 108];
        let dark = [60u8, 20, 14];
        let mut crop = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = ((x as f64 - 45.0) / 42.0, (y as f64 - 25.0) / 22.0);
                if dx * dx + dy * dy <= 1.0 {
                    let i = (y * w + x) * 4;
                    let on_line = (x as i32 - 10 - ((y as i32 - 5) * 2)).abs() <= 1;
                    crop[i..i + 3].copy_from_slice(if on_line { &dark } else { &skin });
                    crop[i + 3] = 255;
                }
            }
        }
        let shapes = trace_mouth(&crop, w, h, &[skin, dark], Color::from_rgb8(60, 20, 14), 1.0);
        assert!(!shapes.is_empty());
        let fills: Vec<_> = shapes.iter().filter(|s| s.fill.is_some()).collect();
        assert!(fills.len() >= 2, "skin and the line both traced");
        for shape in fills {
            assert!(worst_closing_gap(&shape.path) < 3.0, "closed by a chord of {}", worst_closing_gap(&shape.path));
        }
    }
}
