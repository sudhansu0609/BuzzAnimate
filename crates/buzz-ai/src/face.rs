//! Face rigging: eyes (whites, pupils, eyelids with blink and look_at) and restyled viseme mouth.
//!
//! Chunk C6 specifications:
//! - Eyes: the character's DRAWN eye artwork (the traced shapes `assemble_character` picks out
//!   of the parts, see [`DrawnEyes`]) is split into the eye white (light fills and outlines) and
//!   the pupil (dark fills), both layers of the composite eye symbol.
//!   - Blink: an eyelid frame built from the union of the drawn eye whites (the shut lid is that
//!     shape in skin colour, outlined; half is its upper half). Without a drawn white, an ellipse
//!     lid sized by the landmarks.
//!   - Look At: control that moves the pupils within the eye mask bounds.
//! - Mouth: traced mouth is the "Rest" viseme. The other visemes come from `buzz_act::lipsync::placeholder_mouth`,
//!   restyled in the character's line colour and uniform width and scaled to the traced mouth's box.
//!   The mouth symbol plugs into the existing lip-sync `write_track`.

use std::sync::Arc;

use kurbo::{Affine, BezPath, Ellipse, PathEl, Point, Rect, Shape as _};
use peniko::Color;

use buzz_act::lipsync::{write_track, LipSyncReport};
use buzz_audio::{Viseme, VisemeTrack};
use buzz_geom::{boolean, union_all, BoolOp, BooleanOptions, Size};
use buzz_scene::{
    Layer, LayerId, LayerKind, LoopMode, Object, ObjectId, ObjectKind, Scene,
    ShapeData, Symbol, SymbolId,
};

use crate::character::{Skeleton, TracedCharacter, TracedPart};

pub mod mouth_chart;

/// Collection of face Graphic symbols generated for a character.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceRigSymbols {
    pub left_eyelid: Symbol,
    pub left_eye: Symbol,
    pub right_eyelid: Symbol,
    pub right_eye: Symbol,
    pub mouth: Symbol,
    pub left_eye_center: Point,
    pub right_eye_center: Point,
    pub mouth_center: Point,
    /// What the traced mouth part drew that is *not* the mouth: the skin and the
    /// close-the-cut base fill across the part's whole mask, which reaches far past
    /// the lips. Kept out of the mouth symbol (it would stretch the Rest frame, and
    /// with it every scaled viseme and a mouth chart, to the width of the face) and
    /// bound to the head beneath the mouth instead, so nothing round the mouth is
    /// left unpainted. In the character's own coordinates, as drawn. Empty when the
    /// mouth part was kept whole (no mouth landmarks to tell the mouth by).
    pub mouth_underlay: Vec<ShapeData>,
    pub max_pupil_travel: (f64, f64),
}

/// The traced shapes that make up each DRAWN eye, as picked out of the character's
/// parts by `character::assemble_character` (a shape whose centroid lies in the eye's
/// box and which lies at least 80 % inside it) and taken out of those parts. In the
/// character's own coordinates, as traced. Both empty: the character has no drawn eye
/// to keep, and [`build_face_rig`] falls back to a synthetic pupil and an ellipse lid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawnEyes {
    pub left: Vec<ShapeData>,
    pub right: Vec<ShapeData>,
    /// The INK shapes (brush outlines, `TracedPart::ink`) picked out the same way: they
    /// are the eye's outline, so they go to the eye's "White" layer on top of the whites,
    /// whatever their lightness, and never to the pupil (the pupil comes from the fills).
    pub left_ink: Vec<ShapeData>,
    pub right_ink: Vec<ShapeData>,
}

/// Fill lightness (CIE L*) from which a drawn eye shape is eye white (and highlight);
/// darker fills are the pupil / iris.
const EYE_WHITE_MIN_LSTAR: f64 = 45.0;
/// A mouth-part shape is a mouth feature when at least this share of its
/// bounds lies in the (padded) lip-landmark box. See `split_mouth_shapes`.
const MOUTH_FEATURE_MIN_INSIDE: f64 = 0.6;

/// Builds the Eyelid multi-frame graphic symbol (an ellipse lid, for an eye with no
/// drawn white to shape it from; see [`build_eyelid_symbol_from_shape`]):
/// Frame 0: Open (empty)
/// Frame 1: Half-blink (curved eyelid down to midline)
/// Frame 2: Closed (closed eye curve through midline)
pub fn build_eyelid_symbol(
    sym_id: SymbolId,
    name: &str,
    eye_radius: (f64, f64),
    skin_color: Color,
    line_color: Color,
    line_width: f64,
) -> Symbol {
    let mut sym = Symbol::new(sym_id, name.to_string(), buzz_scene::SymbolKind::Graphic);
    sym.registration = Point::ORIGIN;

    let (rx, ry) = eye_radius;
    let mut layer = Layer::new(LayerId(1), "Eyelid", LayerKind::Normal);

    // Frame 0: Open (empty frame)
    layer.frames.insert_keyframe(0);
    layer.frames.set_objects(0, Vec::new());

    // Frame 1: Half-blink (skin-toned arc covering top half down to y = 0)
    layer.frames.insert_frame(1);
    layer.frames.insert_keyframe(1);
    let mut half_path = BezPath::new();
    half_path.move_to(Point::new(-rx, 0.0));
    half_path.curve_to(
        Point::new(-rx, -ry),
        Point::new(rx, -ry),
        Point::new(rx, 0.0),
    );
    half_path.quad_to(Point::new(0.0, ry * 0.2), Point::new(-rx, 0.0));
    half_path.close_path();

    let half_fill = Object::shape(
        ObjectId(sym_id.0 * 1000 + 10),
        ShapeData::filled(half_path.clone(), skin_color),
    );
    let half_stroke = Object::shape(
        ObjectId(sym_id.0 * 1000 + 11),
        ShapeData::stroked(half_path, line_color, line_width),
    );
    layer.frames.set_objects(1, vec![Arc::new(half_fill), Arc::new(half_stroke)]);

    // Frame 2: Fully closed (smooth curve through horizontal midline)
    layer.frames.insert_frame(2);
    layer.frames.insert_keyframe(2);
    let closed_fill_path = Ellipse::new(Point::ORIGIN, (rx, ry), 0.0).to_path(0.05);
    let closed_fill = Object::shape(
        ObjectId(sym_id.0 * 1000 + 20),
        ShapeData::filled(closed_fill_path, skin_color),
    );

    // Midline crease / lash curve
    let mut lash_path = BezPath::new();
    lash_path.move_to(Point::new(-rx * 0.95, 0.0));
    lash_path.quad_to(Point::new(0.0, ry * 0.4), Point::new(rx * 0.95, 0.0));
    let closed_lash = Object::shape(
        ObjectId(sym_id.0 * 1000 + 21),
        ShapeData::stroked(lash_path, line_color, line_width * 1.2),
    );
    layer.frames.set_objects(2, vec![Arc::new(closed_fill), Arc::new(closed_lash)]);

    sym.layers.push_front(layer);
    sym
}

/// Builds the Eyelid multi-frame graphic symbol from the shape of the eye (the union
/// of its drawn whites, centred on the symbol origin):
/// Frame 0: Open (empty)
/// Frame 1: Half-blink (the shape's upper half, skin colour, outlined)
/// Frame 2: Closed (the whole shape, skin colour, outlined)
pub fn build_eyelid_symbol_from_shape(
    sym_id: SymbolId,
    name: &str,
    eye_shape: &BezPath,
    skin_color: Color,
    line_color: Color,
    line_width: f64,
) -> Symbol {
    let mut sym = Symbol::new(sym_id, name.to_string(), buzz_scene::SymbolKind::Graphic);
    sym.registration = Point::ORIGIN;

    let bounds = eye_shape.bounding_box();
    let mut layer = Layer::new(LayerId(1), "Eyelid", LayerKind::Normal);

    // Frame 0: Open (empty frame)
    layer.frames.insert_keyframe(0);
    layer.frames.set_objects(0, Vec::new());

    // Frame 1: Half-blink (the half of the shape above its bounding-box centre; y grows downwards)
    layer.frames.insert_frame(1);
    layer.frames.insert_keyframe(1);
    let upper = Rect::new(bounds.x0 - 1.0, bounds.y0 - 1.0, bounds.x1 + 1.0, bounds.center().y).to_path(0.1);
    let half_path = boolean(
        eye_shape,
        &upper,
        BoolOp::Intersect,
        BooleanOptions::for_shape_size(bounds.width().hypot(bounds.height())),
    );
    let half_objects: Vec<Arc<Object>> = if half_path.elements().is_empty() {
        Vec::new()
    } else {
        vec![
            Arc::new(Object::shape(
                ObjectId(sym_id.0 * 1000 + 10),
                ShapeData::filled(half_path.clone(), skin_color),
            )),
            Arc::new(Object::shape(
                ObjectId(sym_id.0 * 1000 + 11),
                ShapeData::stroked(half_path, line_color, line_width),
            )),
        ]
    };
    layer.frames.set_objects(1, half_objects);

    // Frame 2: Closed (the whole shape)
    layer.frames.insert_frame(2);
    layer.frames.insert_keyframe(2);
    let closed_fill = Object::shape(
        ObjectId(sym_id.0 * 1000 + 20),
        ShapeData::filled(eye_shape.clone(), skin_color),
    );
    let closed_outline = Object::shape(
        ObjectId(sym_id.0 * 1000 + 21),
        ShapeData::stroked(eye_shape.clone(), line_color, line_width),
    );
    layer.frames.set_objects(2, vec![Arc::new(closed_fill), Arc::new(closed_outline)]);

    sym.layers.push_front(layer);
    sym
}

/// Builds the composite Eye symbol containing:
/// - Eye white (layer 1: the drawn white shapes, or nothing)
/// - Pupil (layer 2: ONE object, a shape or a group of shapes, which look_at translates)
/// - Eyelid instance (layer 3, with blink frame selection)
///
/// `white_objects` and `pupil_object` are drawn relative to the symbol origin, which
/// is the eye centre (`center`, where the instance is placed).
pub fn build_composite_eye_symbol(
    sym_id: SymbolId,
    name: &str,
    white_objects: Vec<Arc<Object>>,
    pupil_object: Arc<Object>,
    eyelid_id: SymbolId,
    center: Point,
) -> Symbol {
    let mut sym = Symbol::new(sym_id, name.to_string(), buzz_scene::SymbolKind::Graphic);
    sym.registration = center;

    // Layer 1: Eye White
    let mut white_layer = Layer::new(LayerId(1), "White", LayerKind::Normal);
    white_layer.frames.set_objects(0, white_objects);
    sym.layers.push_front(white_layer);

    // Layer 2: Pupil
    let mut pupil_layer = Layer::new(LayerId(2), "Pupil", LayerKind::Normal);
    pupil_layer.frames.set_objects(0, vec![pupil_object]);
    sym.layers.push_front(pupil_layer);

    // Layer 3: Eyelid
    let mut eyelid_layer = Layer::new(LayerId(3), "Eyelid", LayerKind::Normal);
    let mut eyelid_inst = Object::instance_of(ObjectId(sym_id.0 * 1000 + 3), eyelid_id);
    if let ObjectKind::Instance(inst) = &mut eyelid_inst.kind {
        inst.loop_mode = LoopMode::SingleFrame;
        inst.first_frame = 0; // Open
    }
    eyelid_layer.frames.set_objects(0, vec![Arc::new(eyelid_inst)]);
    sym.layers.push_front(eyelid_layer);

    sym
}

/// Restyles mouth visemes:
/// Frame 0 (Rest): the character's traced mouth artwork.
/// Frames 1..=9: the other 9 visemes from `buzz_act::lipsync`, scaled to match the traced
/// mouth box and restyled in the character's line colour and uniform width.
pub fn restyle_mouth_visemes(
    sym_id: SymbolId,
    name: &str,
    traced_mouth: Option<&TracedPart>,
    head_bounds: Rect,
    palette: &[Color],
    uniform_width: f64,
) -> (Symbol, Point) {
    restyle_mouth_visemes_at(sym_id, name, traced_mouth, None, head_bounds, palette, uniform_width)
}

/// [`restyle_mouth_visemes`] with `mouth_box`, where the mouth is according to the
/// detected landmarks: the position and size to use when there is no traced mouth
/// to measure (without it the mouth would be put by a fraction of the head, which
/// is nowhere near it for a head box taken from the top of the head).
pub fn restyle_mouth_visemes_at(
    sym_id: SymbolId,
    name: &str,
    traced_mouth: Option<&TracedPart>,
    mouth_box: Option<Rect>,
    head_bounds: Rect,
    palette: &[Color],
    uniform_width: f64,
) -> (Symbol, Point) {
    let line_col = palette.first().copied().unwrap_or(Color::from_rgb8(15, 15, 15));
    let mouth_interior = Color::from_rgb8(0x28, 0x14, 0x14);

    let (mouth_center, mouth_w, mouth_h) = if let Some(part) = traced_mouth {
        let mut b: Option<Rect> = None;
        for s in &part.shapes {
            let bb = s.path.bounding_box();
            b = Some(match b {
                Some(all) => all.union(bb),
                None => bb,
            });
        }
        let b = b.unwrap_or(Rect::new(0.0, 0.0, 30.0, 10.0));
        (b.center(), b.width().max(12.0), b.height().max(4.0))
    } else if let Some(b) = mouth_box {
        (b.center(), b.width().max(12.0), b.height().max(4.0))
    } else {
        let c = Point::new(head_bounds.center().x, head_bounds.max_y() - head_bounds.height() * 0.25);
        (c, 28.0, 8.0)
    };

    let mut sym = Symbol::new(sym_id, name.to_string(), buzz_scene::SymbolKind::Graphic);
    sym.registration = mouth_center;

    let mut layer = Layer::new(LayerId(1), "Mouth Shapes", LayerKind::Normal);

    // Visemes specs (standard placeholder dimensions)
    let standard_shapes: [(Viseme, f64, f64); 10] = [
        (Viseme::Rest, 26.0, 3.0),
        (Viseme::Ai, 30.0, 26.0),
        (Viseme::E, 30.0, 14.0),
        (Viseme::O, 20.0, 22.0),
        (Viseme::U, 14.0, 14.0),
        (Viseme::L, 26.0, 18.0),
        (Viseme::WQ, 16.0, 12.0),
        (Viseme::MBP, 26.0, 4.0),
        (Viseme::FV, 24.0, 8.0),
        (Viseme::Etc, 26.0, 12.0),
    ];

    let sx = (mouth_w / 26.0).clamp(0.4, 3.0);
    let sy = (mouth_h / 8.0).clamp(0.4, 3.0);

    for (viseme, std_w, std_h) in standard_shapes {
        let frame = viseme.frame();
        while layer.frames.length() <= frame {
            layer.frames.insert_frame(layer.frames.length());
        }
        layer.frames.insert_keyframe(frame);

        let has_traced_shapes = traced_mouth
            .map(|m| !m.shapes.is_empty())
            .unwrap_or(false);

        let frame_objs: Vec<Arc<Object>> = if viseme == Viseme::Rest && has_traced_shapes {
            // Frame 0: Exact traced mouth artwork translated relative to registration
            let shift = Affine::translate((-mouth_center.x, -mouth_center.y));
            traced_mouth
                .unwrap()
                .shapes
                .iter()
                .enumerate()
                .map(|(si, s)| {
                    let mut s_shifted = s.clone();
                    s_shifted.path = shift * s_shifted.path;
                    let obj = Object::shape(ObjectId(sym_id.0 * 1000 + si as u64), s_shifted);
                    Arc::new(obj)
                })
                .collect()
        } else if viseme == Viseme::Rest {
            // No traced mouth: the drawn mouth is on the head (the underlay),
            // so Rest draws nothing rather than a dark lens over the smile.
            Vec::new()
        } else {
            // Generated restyled viseme shape scaled to character mouth dimensions
            let w = (std_w * sx).max(6.0);
            let h = (std_h * sy).max(2.0);
            let path = Ellipse::new(Point::ZERO, (w / 2.0, h / 2.0), 0.0).to_path(0.05);

            let fill_obj = Object::shape(
                ObjectId(sym_id.0 * 1000 + frame as u64 * 10 + 1),
                ShapeData::filled(path.clone(), mouth_interior),
            );
            let stroke_obj = Object::shape(
                ObjectId(sym_id.0 * 1000 + frame as u64 * 10 + 2),
                ShapeData::stroked(path, line_col, uniform_width),
            );
            vec![Arc::new(fill_obj), Arc::new(stroke_obj)]
        };

        layer.frames.set_objects(frame, frame_objs);
    }

    sym.layers.push_front(layer);
    (sym, mouth_center)
}

/// One eye's placement, worked out from its detected landmark contour.
struct EyeGeometry {
    center: Point,
    /// (rx, ry) for the synthetic eyelid/pupil (an eye with no drawn white).
    half_size: (f64, f64),
    /// The box the drawn eye artwork is selected by
    /// (`character::assemble_character`) and the skin / fallback pupil colours
    /// are sampled from: the landmark box, widened to also cover a
    /// same-side dedicated "eye" traced part's own extent, since a keypoint
    /// estimate and a segmentation estimate of "the eye" do not exactly
    /// agree on this character.
    removal_box: Rect,
}

/// Centre, size and removal box for one eye. Falls back to the old
/// head-fraction heuristic only when this eye's landmarks are missing or
/// all low-confidence.
fn eye_geometry(
    points: &[(f32, f32, f32)],
    side_sign: f64,
    side_name: &str,
    head_pt: Point,
    head_radius: f64,
    traced: &TracedCharacter,
) -> EyeGeometry {
    let confident: Vec<Point> = points
        .iter()
        .filter(|p| p.2 > 0.1)
        .map(|p| Point::new(p.0 as f64, p.1 as f64))
        .collect();

    let (center, half_size, mut removal_box) = if confident.len() >= 2 {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &confident {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        let landmark_box = Rect::new(x0, y0, x1, y1);
        let c = landmark_box.center();
        // A little larger than the raw landmark extent, which traces just
        // the contour points rather than the true eye-white edge.
        let pad = 1.2;
        let hx = ((landmark_box.width() * 0.5) * pad).clamp(4.0, 30.0);
        let hy = ((landmark_box.height() * 0.5) * pad).clamp(3.0, 20.0);
        (c, (hx, hy), landmark_box)
    } else {
        // Landmarks missing or low-confidence: the previous heuristic.
        let c = Point::new(head_pt.x + side_sign * head_radius * 0.35, head_pt.y - head_radius * 0.1);
        let hx = (head_radius * 0.22).clamp(6.0, 30.0);
        let hy = (head_radius * 0.15).clamp(4.0, 20.0);
        (c, (hx, hy), Rect::from_center_size(c, Size::new(hx * 2.0, hy * 2.0)))
    };

    // Union in the dedicated traced eye part's own bounding box, when this
    // character has one, so its shapes fall entirely inside the removal box
    // regardless of any offset between the keypoint and segmentation
    // estimates of "the eye".
    for part in &traced.parts {
        let n = part.name.to_lowercase();
        if n.contains("eye") && !n.contains("brow") && n.contains(side_name) {
            for s in &part.shapes {
                removal_box = removal_box.union(s.path.bounding_box());
            }
        }
    }

    EyeGeometry { center, half_size, removal_box }
}

/// The box each eye's drawn artwork is selected by before assembly
/// (`character::assemble_character`): the shapes in it are taken out of the
/// parts and handed to [`build_face_rig`] as [`DrawnEyes`], which builds the
/// white, pupil and lid of the composite eye from them.
pub fn eye_removal_boxes(skel: &Skeleton, traced: &TracedCharacter) -> (Rect, Rect) {
    let head_pt = skel.joint("Head").map(|j| Point::new(j.x as f64, j.y as f64)).unwrap_or(Point::new(250.0, 150.0));
    let head_radius = (skel.character_height as f64 * 0.1).max(25.0);
    let left = eye_geometry(&skel.face.left_eye, -1.0, "left", head_pt, head_radius, traced);
    let right = eye_geometry(&skel.face.right_eye, 1.0, "right", head_pt, head_radius, traced);
    (left.removal_box, right.removal_box)
}

/// Lightest and darkest fill colours among traced shapes overlapping `box_`
/// — used for the colour of the synthetic pupil of an eye with no drawn pupil
/// (`build_one_eye`), so it reads as *this* character's. Falls back to
/// `fallback_white`/`fallback_pupil` when nothing overlaps.
fn sample_eye_colors(
    traced: &TracedCharacter,
    box_: Rect,
    fallback_white: Color,
    fallback_pupil: Color,
) -> (Color, Color) {
    let mut lightest: Option<(f32, Color)> = None;
    let mut darkest: Option<(f32, Color)> = None;
    for part in &traced.parts {
        for s in &part.shapes {
            let Some(fill) = &s.fill else { continue };
            let b = s.path.bounding_box();
            let overlaps = b.x0 < box_.x1 && b.x1 > box_.x0 && b.y0 < box_.y1 && b.y1 > box_.y0;
            if !overlaps {
                continue;
            }
            let c = fill.color();
            let rgba = c.to_rgba8();
            let lum = 0.299 * rgba.r as f32 + 0.587 * rgba.g as f32 + 0.114 * rgba.b as f32;
            if lightest.as_ref().map(|(l, _)| lum > *l).unwrap_or(true) {
                lightest = Some((lum, c));
            }
            if darkest.as_ref().map(|(l, _)| lum < *l).unwrap_or(true) {
                darkest = Some((lum, c));
            }
        }
    }
    (
        lightest.map(|(_, c)| c).unwrap_or(fallback_white),
        darkest.map(|(_, c)| c).unwrap_or(fallback_pupil),
    )
}

/// The box of the confidently detected mouth landmarks (fewer than three: `None`).
/// The box a Mouth-part shape must (mostly) lie in to be a mouth FEATURE: the
/// lip landmark box padded 20 % + 3 px sideways and 60 % + 4 px vertically.
/// Shared by [`split_mouth_shapes`] and the character pipeline, which hands
/// the Mouth part's other shapes to the head.
/// A shape is a mouth FEATURE when its bounds' centre lies in `mouth_box` and
/// most of its bounds (by area, [`MOUTH_FEATURE_MIN_INSIDE`]) do: a smile drawn
/// a little wider than the lip landmarks still counts, the skin patch round it
/// does not. The part's ink (the drawn smile is ink with the colour tracer)
/// is judged the same way.
pub(crate) fn is_mouth_feature(shape: &ShapeData, mouth_box: Rect) -> bool {
    let b = shape.path.bounding_box();
    if !mouth_box.contains(b.center()) {
        return false;
    }
    let inter = b.intersect(mouth_box);
    inter.area() / b.area().max(1e-6) >= MOUTH_FEATURE_MIN_INSIDE
}

pub(crate) fn mouth_feature_box(landmarks: &[(f32, f32, f32)]) -> Option<Rect> {
    let lips = mouth_landmark_box(landmarks)?;
    let (pad_x, pad_y) = (lips.width() * 0.2 + 3.0, lips.height() * 0.6 + 4.0);
    Some(Rect::new(lips.x0 - pad_x, lips.y0 - pad_y, lips.x1 + pad_x, lips.y1 + pad_y))
}

pub(crate) fn mouth_landmark_box(landmarks: &[(f32, f32, f32)]) -> Option<Rect> {
    let points: Vec<Point> = landmarks
        .iter()
        .filter(|p| p.2 > 0.2)
        .map(|p| Point::new(p.0 as f64, p.1 as f64))
        .collect();
    if points.len() < 3 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &points {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    Some(Rect::new(x0, y0, x1, y1))
}

/// **Splits the traced mouth part into the mouth and what lies round it.**
///
/// The part holds every shape traced inside its *mask*, a patch of the face well
/// past the lips: the close-the-cut base fill across the whole mask and the skin
/// clipped to it, as well as the lips, teeth, inside of the mouth and the line
/// round it. Taken whole as the Rest frame, its box is the width of the face (129
/// px on a 135 px face), and everything scaled to it (the placeholder visemes, a
/// mouth chart) comes out that wide. So: the shapes whose box lies within the box
/// of the detected mouth landmarks (a little padded) are the mouth, and are
/// returned as a part of their own for the symbol; every other shape is the
/// *underlay*, to be bound to the head beneath the mouth so the patch stays painted.
///
/// With fewer than three confident landmarks there is nothing to tell the mouth by:
/// the part is returned whole, with no underlay. With landmarks but no shape inside
/// their box, there is no traced mouth (`None`) and every shape is underlay.
fn split_mouth_shapes(part: Option<&TracedPart>, landmarks: &[(f32, f32, f32)]) -> (Option<TracedPart>, Vec<ShapeData>) {
    let Some(part) = part else { return (None, Vec::new()) };
    let Some(mouth_box) = mouth_feature_box(landmarks) else {
        return (Some(part.clone()), Vec::new());
    };
    let is_feature = |shape: &ShapeData| is_mouth_feature(shape, mouth_box);
    let (mut features, mut underlay) = (Vec::new(), Vec::new());
    for shape in part.shapes.iter().chain(&part.ink) {
        if is_feature(shape) {
            features.push(shape.clone());
        } else {
            underlay.push(shape.clone());
        }
    }
    if std::env::var_os("BUZZ_FACE_LOG").is_some() {
        eprintln!(
            "split_mouth_shapes: box ({:.0},{:.0})-({:.0},{:.0}), {} shapes + {} ink → {} features, {} underlay",
            mouth_box.x0,
            mouth_box.y0,
            mouth_box.x1,
            mouth_box.y1,
            part.shapes.len(),
            part.ink.len(),
            features.len(),
            underlay.len()
        );
        for s in part.shapes.iter().chain(&part.ink) {
            let b = s.path.bounding_box();
            eprintln!(
                "  {:?} area {:.0} bbox ({:.0},{:.0})-({:.0},{:.0}) feature {}",
                s.fill.as_ref().map(|f| f.color().to_rgba8().to_u8_array()),
                s.path.area().abs(),
                b.x0,
                b.y0,
                b.x1,
                b.y1,
                is_feature(s)
            );
        }
    }
    if features.is_empty() {
        return (None, underlay);
    }
    (
        Some(TracedPart { name: part.name.clone(), depth: part.depth, shapes: features, ink: Vec::new(), caps: Vec::new() }),
        underlay,
    )
}

/// CIE L* (0 black .. 100 white) of an sRGB colour.
fn lightness(color: Color) -> f64 {
    let rgba = color.to_rgba8();
    let linear = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    let y = 0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b);
    let f = if y > 216.0 / 24389.0 { y.cbrt() } else { (24389.0 / 27.0 * y + 16.0) / 116.0 };
    116.0 * f - 16.0
}

/// A shape's lightness: its fill's, else its stroke's (white if it paints nothing).
fn shape_lightness(shape: &ShapeData) -> f64 {
    match (&shape.fill, &shape.stroke) {
        (Some(fill), _) => lightness(fill.color()),
        (None, Some(stroke)) => lightness(stroke.color()),
        (None, None) => 100.0,
    }
}

/// `path` with its holes taken out: only the contours wound like the largest one
/// (the outside, and any island inside a hole). A traced eye white has a hole where
/// the pupil is drawn; the shut lid must cover the pupil, not frame it.
fn outer_contours(path: &BezPath) -> BezPath {
    let mut subpaths: Vec<BezPath> = Vec::new();
    let mut current = BezPath::new();
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) && !current.elements().is_empty() {
            subpaths.push(std::mem::take(&mut current));
        }
        current.push(*el);
    }
    if !current.elements().is_empty() {
        subpaths.push(current);
    }
    let areas: Vec<f64> = subpaths.iter().map(|s| s.area()).collect();
    let Some(largest) = areas.iter().copied().max_by(|a, b| a.abs().total_cmp(&b.abs())) else {
        return path.clone();
    };
    let mut out = BezPath::new();
    for (sub, area) in subpaths.iter().zip(&areas) {
        if area.signum() == largest.signum() {
            for el in sub.elements() {
                out.push(*el);
            }
        }
    }
    out
}

/// One eye's drawn artwork, split into its layers and moved so the eye centre is the
/// origin (see [`split_drawn_eye`]).
struct EyeArtwork {
    /// The eye centre in character coordinates: the centre of the union of the white
    /// shapes' bounding box, else the landmark centre.
    centre: Point,
    half_size: (f64, f64),
    /// Light but coloured fills (the skin piece round the eye socket): drawn
    /// under everything else in the eye.
    socket: Vec<ShapeData>,
    /// Light fills (white, highlights) and outline strokes, as drawn.
    white: Vec<ShapeData>,
    /// Only when the whites have a hole (where the iris sits): the whites without it,
    /// to go behind them, so a pupil that looks aside does not uncover the skin.
    backing: Option<ShapeData>,
    /// Dark fills (pupil, iris), as drawn.
    pupil: Vec<ShapeData>,
    /// The union of the white fills without holes: the shape the lid is cut from.
    lid: Option<BezPath>,
}

/// A light fill narrower than this on average (`2 · area / perimeter`, px) is
/// a line (a lid highlight the colour tracer draws as a thin light shape) or a
/// glint, not an eye white: it is drawn but never counted in the whites'
/// union the lid is cut from. 2 px: a small character's white drawn as a
/// ring round its pupil (the face-rig fixture: 22 × 8 px) is 3 px wide.
const EYE_WHITE_MIN_MEAN_WIDTH_PX: f64 = 2.0;

/// `2 · area / perimeter`: the mean width of a filled shape.
fn mean_width(path: &BezPath) -> f64 {
    let perimeter = path.perimeter(0.1);
    if perimeter <= 1e-9 {
        return 0.0;
    }
    2.0 * path.area().abs() / perimeter
}

/// An eye white is light AND near-neutral: CIELAB chroma (`√(a² + b²)`) at
/// most this. The skin round the eye socket (L* 67, chroma ≈ 53) is light too,
/// and with the colour tracer the Eye part gets a skin-coloured piece of the
/// head's skin fill the size of the eye; counted as a white it became the
/// biggest "white", so the backing under the pupil was painted SKIN (a skin
/// ring round the pupil) and the lid was cut round the socket.
const EYE_WHITE_MAX_CHROMA: f64 = 20.0;

/// CIELAB chroma of an sRGB colour.
fn chroma(color: Color) -> f64 {
    let c = color.to_rgba8();
    let lab = buzz_scene::trace::srgb_to_lab([c.r, c.g, c.b]);
    (lab[1] * lab[1] + lab[2] * lab[2]).sqrt()
}

/// A shape's chroma: its fill's, else its stroke's (0 if it paints nothing).
fn shape_chroma(shape: &ShapeData) -> f64 {
    match (&shape.fill, &shape.stroke) {
        (Some(fill), _) => chroma(fill.color()),
        (None, Some(stroke)) => chroma(stroke.color()),
        (None, None) => 0.0,
    }
}

/// Light and near-neutral: an eye white (or a glint).
fn is_eye_white(shape: &ShapeData) -> bool {
    shape_lightness(shape) >= EYE_WHITE_MIN_LSTAR && shape_chroma(shape) <= EYE_WHITE_MAX_CHROMA
}

fn split_drawn_eye(drawn: &[ShapeData], ink: &[ShapeData], landmark_centre: Point, landmark_half: (f64, f64)) -> EyeArtwork {
    let light_fills: Vec<&ShapeData> = drawn
        .iter()
        .filter(|s| s.fill.is_some() && is_eye_white(s) && mean_width(&s.path) >= EYE_WHITE_MIN_MEAN_WIDTH_PX)
        .collect();
    if std::env::var_os("BUZZ_FACE_LOG").is_some() {
        for s in drawn {
            let b = s.path.bounding_box();
            eprintln!(
                "split_drawn_eye: fill {:?} L* {:.0} area {:.0} mean width {:.1} bbox ({:.0},{:.0})-({:.0},{:.0})",
                s.fill.as_ref().map(|f| f.color().to_rgba8().to_u8_array()),
                shape_lightness(s),
                s.path.area().abs(),
                mean_width(&s.path),
                b.x0,
                b.y0,
                b.x1,
                b.y1
            );
        }
        eprintln!("split_drawn_eye: {} ink shapes", ink.len());
    }
    let paths: Vec<BezPath> = light_fills.iter().map(|s| s.path.clone()).collect();
    let mut union = BezPath::new();
    if !paths.is_empty() {
        let all = paths.iter().map(|p| p.bounding_box()).reduce(|a, b| a.union(b)).unwrap();
        union = union_all(&paths, BooleanOptions::for_shape_size(all.width().hypot(all.height())));
    }
    let ub = union.bounding_box();
    let has_white = !union.elements().is_empty()
        && ub.width().is_finite()
        && ub.height().is_finite()
        && ub.width() > 0.5
        && ub.height() > 0.5;
    let (centre, half_size) = if has_white {
        (ub.center(), (ub.width() * 0.5, ub.height() * 0.5))
    } else {
        (landmark_centre, landmark_half)
    };

    let shift = Affine::translate((-centre.x, -centre.y));
    let moved = |s: &ShapeData| {
        let mut m = s.clone();
        m.path = shift * m.path;
        m
    };
    // Socket: light but coloured fills (the skin piece round the eye) go to
    // the very bottom of the white layer — under the backing and the whites,
    // never in the lid's union — so the head's skin is still filled where the
    // Eye part owns it, and nothing of it shows through the eye.
    let (mut socket, mut white, mut pupil) = (Vec::new(), Vec::new(), Vec::new());
    for s in drawn {
        // An outline stroke belongs to the white whatever its colour: it is the eye's edge.
        if s.fill.is_none() || is_eye_white(s) {
            white.push(moved(s));
        } else if shape_lightness(s) >= EYE_WHITE_MIN_LSTAR {
            socket.push(moved(s));
        } else {
            pupil.push(moved(s));
        }
    }
    // The ink is the eye's outline: on top of the whites, never the pupil.
    white.extend(ink.iter().map(moved));

    let (mut lid, mut backing) = (None, None);
    if has_white {
        let outer = outer_contours(&union);
        let has_hole = outer.area().abs() > union.area().abs() * 1.005 + 0.5;
        let lid_path = shift * outer;
        if has_hole {
            let biggest = light_fills
                .iter()
                .max_by(|a, b| a.path.area().abs().total_cmp(&b.path.area().abs()))
                .and_then(|s| s.fill.as_ref())
                .map(|f| f.color());
            if let Some(colour) = biggest {
                backing = Some(ShapeData::filled(lid_path.clone(), colour));
            }
        }
        lid = Some(lid_path);
    }
    // The socket goes first in the white layer: `backing` (when any) is drawn
    // before `white` by the caller, so the socket is prepended to the backing
    // by making it part of the white list's head only when there is no
    // backing; otherwise it rides below the backing via `socket_under`.
    EyeArtwork { centre, half_size, socket, white, backing, pupil, lid }
}

/// The skin colour round an eye: the largest fill of the head part (every part when
/// none is called head) that touches the eye's box, not counting the eye's own
/// shapes (those wholly inside the box). `fallback` when nothing does.
fn skin_colour_near(traced: &TracedCharacter, box_: Rect, fallback: Color) -> Color {
    let head_parts: Vec<&TracedPart> =
        traced.parts.iter().filter(|p| p.name.to_lowercase().contains("head")).collect();
    let pool: Vec<&TracedPart> = if head_parts.is_empty() { traced.parts.iter().collect() } else { head_parts };
    // Points just outside the box: a skin fill with a hole where the eye was
    // still covers these.
    let ring = box_.inflate(3.0, 3.0);
    let probes = [
        Point::new(ring.x0, ring.y0),
        Point::new(ring.center().x, ring.y0),
        Point::new(ring.x1, ring.y0),
        Point::new(ring.x0, ring.center().y),
        Point::new(ring.x1, ring.center().y),
        Point::new(ring.x0, ring.y1),
        Point::new(ring.center().x, ring.y1),
        Point::new(ring.x1, ring.y1),
    ];
    let mut best: Option<(f64, Color)> = None;
    for part in pool {
        for s in &part.shapes {
            let Some(fill) = &s.fill else { continue };
            let b = s.path.bounding_box();
            if b.x0 >= box_.x0 && b.x1 <= box_.x1 && b.y0 >= box_.y0 && b.y1 <= box_.y1 {
                continue;
            }
            if !probes.iter().any(|p| s.path.contains(*p)) {
                continue;
            }
            let area = s.path.area().abs();
            if best.map_or(true, |(a, _)| area > a) {
                best = Some((area, fill.color()));
            }
        }
    }
    best.map(|(_, c)| c).unwrap_or(fallback)
}

/// One built eye: its lid symbol, the composite eye symbol, and where it sits.
struct BuiltEye {
    eyelid: Symbol,
    eye: Symbol,
    centre: Point,
    half_size: (f64, f64),
}

#[allow(clippy::too_many_arguments)]
fn build_one_eye(
    lid_id: SymbolId,
    eye_id: SymbolId,
    lid_name: &str,
    eye_name: &str,
    geo: &EyeGeometry,
    drawn: &[ShapeData],
    drawn_ink: &[ShapeData],
    traced: &TracedCharacter,
    line_col: Color,
    default_skin: Color,
    uniform_width: f64,
) -> BuiltEye {
    let art = split_drawn_eye(drawn, drawn_ink, geo.center, geo.half_size);
    let skin_col = skin_colour_near(traced, geo.removal_box, default_skin);
    let eyelid = match &art.lid {
        Some(shape) => build_eyelid_symbol_from_shape(lid_id, lid_name, shape, skin_col, line_col, uniform_width),
        None => build_eyelid_symbol(lid_id, lid_name, art.half_size, skin_col, line_col, uniform_width),
    };

    let base = eye_id.0 * 1000;
    let white_objects: Vec<Arc<Object>> = art
        .socket
        .into_iter()
        .chain(art.backing)
        .chain(art.white)
        .enumerate()
        .map(|(k, s)| Arc::new(Object::shape(ObjectId(base + 10 + k as u64), s)))
        .collect();

    // The pupil layer's first (only) object is what look-at moves: one shape, or
    // ONE group of the drawn pupil shapes. None drawn: the synthetic pupil.
    let mut pupil_shapes = art.pupil;
    let pupil_object = match pupil_shapes.len() {
        0 => {
            let (_, pupil_col) = sample_eye_colors(traced, geo.removal_box, Color::WHITE, line_col);
            let r = (art.half_size.0 * 0.45).clamp(2.5, 12.0);
            let path = Ellipse::new(Point::ORIGIN, (r, r), 0.0).to_path(0.05);
            Object::shape(ObjectId(base + 2), ShapeData::filled(path, pupil_col))
        }
        1 => Object::shape(ObjectId(base + 2), pupil_shapes.remove(0)),
        _ => {
            let children: Vec<Arc<Object>> = pupil_shapes
                .into_iter()
                .enumerate()
                .map(|(k, s)| Arc::new(Object::shape(ObjectId(base + 500 + k as u64), s)))
                .collect();
            Object::group(ObjectId(base + 2), children)
        }
    };

    let eye = build_composite_eye_symbol(eye_id, eye_name, white_objects, Arc::new(pupil_object), lid_id, art.centre);
    BuiltEye { eyelid, eye, centre: art.centre, half_size: art.half_size }
}

/// Builds full Face Rig symbols for a character (Eyes, Eyelids, and restyled 10-frame Mouth).
/// The eyes are built from the character's `drawn_eyes` (see [`DrawnEyes`]).
pub fn build_face_rig(
    skel: &Skeleton,
    traced: &TracedCharacter,
    character_name: &str,
    palette: &[Color],
    uniform_width: f64,
    start_symbol_id: u64,
    drawn_eyes: &DrawnEyes,
) -> FaceRigSymbols {
    let line_col = palette.first().copied().unwrap_or(Color::from_rgb8(15, 15, 15));
    let skin_col = palette.get(palette.len().saturating_sub(2)).copied().unwrap_or(Color::from_rgb8(240, 200, 170));

    // 1. Locate head and facial landmarks
    let head_pt = skel.joint("Head").map(|j| Point::new(j.x as f64, j.y as f64)).unwrap_or(Point::new(250.0, 150.0));
    let head_radius = (skel.character_height as f64 * 0.1).max(25.0);
    let head_bounds = Rect::from_center_size(head_pt, Size::new(head_radius * 2.0, head_radius * 2.5));

    // Eye centre and size, from each eye's own detected landmark contour —
    // not a fixed fraction of head size, which is what produced oversized
    // "googly eye" whites pasted over the drawn face. Falls back to the old
    // heuristic only when landmarks are missing or low-confidence.
    let left_eye_geo = eye_geometry(&skel.face.left_eye, -1.0, "left", head_pt, head_radius, traced);
    let right_eye_geo = eye_geometry(&skel.face.right_eye, 1.0, "right", head_pt, head_radius, traced);

    let id_base = start_symbol_id;

    // Each eye from its drawn artwork: the white layer and the pupil layer are the
    // traced shapes, the lid is cut from the union of the whites (symbol ids 1, 2, 5,
    // 6 are the old synthetic white and pupil symbols, left unused so the others keep
    // their numbers).
    let left = build_one_eye(
        SymbolId(id_base + 3),
        SymbolId(id_base + 4),
        &format!("{}_eyelid_l", character_name),
        &format!("{}_eye_l", character_name),
        &left_eye_geo,
        &drawn_eyes.left,
        &drawn_eyes.left_ink,
        traced,
        line_col,
        skin_col,
        uniform_width,
    );
    let right = build_one_eye(
        SymbolId(id_base + 7),
        SymbolId(id_base + 8),
        &format!("{}_eyelid_r", character_name),
        &format!("{}_eye_r", character_name),
        &right_eye_geo,
        &drawn_eyes.right,
        &drawn_eyes.right_ink,
        traced,
        line_col,
        skin_col,
        uniform_width,
    );
    let (left_eye_center, right_eye_center) = (left.centre, right.centre);
    let max_travel = (
        left.half_size.0.max(right.half_size.0) * 0.4,
        left.half_size.1.max(right.half_size.1) * 0.4,
    );
    let (left_eyelid, left_eye) = (left.eyelid, left.eye);
    let (right_eyelid, right_eye) = (right.eyelid, right.eye);

    // Mouth component
    let traced_mouth = traced.parts.iter().find(|p| p.name.to_lowercase().contains("mouth"));
    let (mouth_features, mouth_underlay) = split_mouth_shapes(traced_mouth, &skel.face.mouth);
    let (mouth_sym, mouth_center) = restyle_mouth_visemes_at(
        SymbolId(id_base + 9),
        &format!("{}_mouth", character_name),
        mouth_features.as_ref(),
        mouth_landmark_box(&skel.face.mouth),
        head_bounds,
        palette,
        uniform_width,
    );

    FaceRigSymbols {
        left_eyelid,
        left_eye,
        right_eyelid,
        right_eye,
        mouth: mouth_sym,
        left_eye_center,
        right_eye_center,
        mouth_center,
        mouth_underlay,
        max_pupil_travel: max_travel,
    }
}

/// Applies eye gaze offset (-1.0..=1.0) to pupil positions inside a composite Eye symbol.
pub fn set_eye_look_at(eye_sym: &mut Symbol, gaze_x: f64, gaze_y: f64, max_travel: (f64, f64)) {
    let dx = gaze_x.clamp(-1.0, 1.0) * max_travel.0;
    let dy = gaze_y.clamp(-1.0, 1.0) * max_travel.1;

    let pupil_layer_id = eye_sym.layers.iter().find(|l| l.name == "Pupil").map(|l| l.id);
    if let Some(id) = pupil_layer_id {
        eye_sym.layers.update(id, |pupil_layer| {
            if let Some(first_obj) = pupil_layer.frames.objects_at(0).first() {
                let mut updated = (**first_obj).clone();
                updated.transform = Affine::translate((dx, dy));
                pupil_layer.frames.set_objects(0, vec![Arc::new(updated)]);
            }
        });
    }
}

/// Sets blink state on a composite Eye symbol:
/// `t`: 0.0 = fully open, 0.5 = half-closed, 1.0 = fully closed.
pub fn set_eye_blink(eye_sym: &mut Symbol, t: f64) {
    let frame = if t < 0.33 {
        0
    } else if t < 0.67 {
        1
    } else {
        2
    };

    let eyelid_layer_id = eye_sym.layers.iter().find(|l| l.name == "Eyelid").map(|l| l.id);
    if let Some(id) = eyelid_layer_id {
        eye_sym.layers.update(id, |eyelid_layer| {
            if let Some(first_obj) = eyelid_layer.frames.objects_at(0).first() {
                let mut updated = (**first_obj).clone();
                if let ObjectKind::Instance(inst) = &mut updated.kind {
                    inst.loop_mode = LoopMode::SingleFrame;
                    inst.first_frame = frame;
                }
                eyelid_layer.frames.set_objects(0, vec![Arc::new(updated)]);
            }
        });
    }
}

/// Helper driving a mouth symbol on a scene layer from a lip-sync VisemeTrack.
pub fn drive_mouth_track(
    scene: &mut Scene,
    mouth_symbol: SymbolId,
    track: &VisemeTrack,
    sound_start: u32,
    layer_id: LayerId,
    placement: Affine,
) -> LipSyncReport {
    write_track(scene, track, sound_start, layer_id, mouth_symbol, placement)
}
