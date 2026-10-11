//! **Title cards**: a line of text animated on and off with one of five
//! presets (T in `docs/PRODUCTION_SPEED_PLAN.md`).
//!
//! Pure: text + preset in, keyed layers out. The text is laid down with
//! `buzz_text` as vector outlines, so Devanagari is shaped. It is cut into
//! **units** — per shaped cluster (Typewriter: a conjunct or a matra is never
//! split), per word (PopWords, Stamp) or the whole line (FadeUp, SlideIn) — and
//! each unit becomes a Graphic symbol instance on a layer of its own inside one
//! card symbol, keyed with classic tweens and eases (no per-frame shape keys).
//!
//! The card symbol is placed once on a new top stage layer that **ignores the
//! lights** (a title sits on the screen, not in the world), from `start` for as
//! long as the card runs, with a blank keyframe after it.

use std::collections::BTreeMap;
use std::sync::Arc;

use buzz_geom::{Affine, BezPath, Point, Rect, Vec2};
use buzz_scene::{
    ColorTransform, Easing, Keyframe, Layer, LayerId, LayerKind, LayerTimeline, LoopMode, Object,
    ObjectKind, Scene, ShapeData, SymbolId, SymbolKind, Tween,
};
use kurbo::Shape as _;
use peniko::Color;

/// How a card comes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitlePreset {
    /// Cluster by cluster, 2 frames each. Out: a fade.
    Typewriter,
    /// Each word scales 0 → 1.15 → 1 over 6 frames, staggered 4. Out: a fade.
    PopWords,
    /// The whole line fades in and rises 4 % of the stage height, easing out.
    /// Out: the reverse.
    #[default]
    FadeUp,
    /// The whole line slides in from the left, easing out with a slight
    /// overshoot. Out: the reverse.
    SlideIn,
    /// Each word scales 1.6 → 1 in 4 frames, then holds 2 before the next.
    /// Out: a fade.
    Stamp,
}

impl TitlePreset {
    pub const ALL: [TitlePreset; 5] = [
        TitlePreset::Typewriter,
        TitlePreset::PopWords,
        TitlePreset::FadeUp,
        TitlePreset::SlideIn,
        TitlePreset::Stamp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TitlePreset::Typewriter => "Typewriter",
            TitlePreset::PopWords => "Pop words",
            TitlePreset::FadeUp => "Fade up",
            TitlePreset::SlideIn => "Slide in",
            TitlePreset::Stamp => "Stamp",
        }
    }

    /// The snake_case name the agent and casting.json use.
    pub fn id(self) -> &'static str {
        match self {
            TitlePreset::Typewriter => "typewriter",
            TitlePreset::PopWords => "pop_words",
            TitlePreset::FadeUp => "fade_up",
            TitlePreset::SlideIn => "slide_in",
            TitlePreset::Stamp => "stamp",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        Self::ALL.into_iter().find(|p| p.id() == id || p.id().replace('_', "") == id)
    }

    fn unit(self) -> Unit {
        match self {
            TitlePreset::Typewriter => Unit::Cluster,
            TitlePreset::PopWords | TitlePreset::Stamp => Unit::Word,
            TitlePreset::FadeUp | TitlePreset::SlideIn => Unit::Line,
        }
    }
}

/// Where on the stage the card sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitlePosition {
    #[default]
    Centre,
    LowerThird,
    Top,
}

impl TitlePosition {
    pub const ALL: [TitlePosition; 3] =
        [TitlePosition::Centre, TitlePosition::LowerThird, TitlePosition::Top];

    pub fn label(self) -> &'static str {
        match self {
            TitlePosition::Centre => "Centre",
            TitlePosition::LowerThird => "Lower third",
            TitlePosition::Top => "Top",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            TitlePosition::Centre => "centre",
            TitlePosition::LowerThird => "lower_third",
            TitlePosition::Top => "top",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        match id.as_str() {
            "centre" | "center" | "middle" => Some(TitlePosition::Centre),
            "lower_third" | "lowerthird" | "bottom" => Some(TitlePosition::LowerThird),
            "top" => Some(TitlePosition::Top),
            _ => None,
        }
    }
}

/// The optional backing plate: a rounded rectangle behind the text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TitlePlate {
    pub color: Color,
    /// 0..1.
    pub opacity: f64,
}

/// Everything a title card is made from.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleSpec {
    pub text: String,
    /// Font family; `None` picks the system default (or a Devanagari face when
    /// the text is Hindi).
    pub font: Option<String>,
    /// Text size in document units.
    pub size: f64,
    pub color: Color,
    pub plate: Option<TitlePlate>,
    pub preset: TitlePreset,
    /// How long the card holds, fully on, between its in and its out.
    pub hold_seconds: f64,
    pub position: TitlePosition,
    /// The stage frame the card starts on.
    pub start: u32,
    /// The card's whole length in frames, in + hold + out, overriding
    /// `hold_seconds` (the hold shrinks to fit, never below 0).
    pub total_frames: Option<u32>,
}

impl TitleSpec {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            font: None,
            size: 64.0,
            color: Color::WHITE,
            plate: None,
            preset: TitlePreset::FadeUp,
            hold_seconds: 2.0,
            position: TitlePosition::Centre,
            start: 0,
            total_frames: None,
        }
    }
}

/// What [`add_title_card`] made.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleCard {
    /// The new top stage layer (ignores the lights).
    pub layer: LayerId,
    /// The card symbol placed on it.
    pub symbol: SymbolId,
    /// First stage frame of the card.
    pub start: u32,
    /// How many frames it runs (in + hold + out); the last is transparent.
    pub frames: u32,
    /// How many animated units the text was cut into.
    pub units: usize,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TitleError {
    #[error("a title card needs some text")]
    NoText,
    #[error("no font could draw that text")]
    NoFont,
    #[error("the text size must be a positive number")]
    BadSize,
}

/// Frames of every preset's out.
pub const OUT_FRAMES: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Cluster,
    Word,
    Line,
}

/// One piece of the text that animates on its own: its text and its
/// outlines, in line coordinates (first baseline at y = 0).
#[derive(Debug, Clone)]
pub struct TitleUnit {
    pub text: String,
    pub path: BezPath,
}

/// The family a card is set in: the one asked for, or — when none is asked for
/// and the text has Devanagari in it — a face that can set Hindi (Nirmala UI
/// first), because the system default is Latin-only.
pub fn family_for(text: &str, font: Option<&str>) -> Option<String> {
    if let Some(f) = font.filter(|f| !f.trim().is_empty()) {
        return Some(f.to_string());
    }
    if !text.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)) {
        return None;
    }
    let families = buzz_text::font_families();
    families
        .iter()
        .find(|f| f.devanagari && f.name.eq_ignore_ascii_case("Nirmala UI"))
        .or_else(|| families.iter().find(|f| f.devanagari))
        .map(|f| f.name.clone())
}

/// The units `preset` animates `text` in — shaped clusters, words or the whole
/// line — with their outlines laid out exactly as the whole line is drawn.
/// Spaces are not units.
pub fn units(text: &str, size: f64, font: Option<&str>, preset: TitlePreset) -> Vec<TitleUnit> {
    let family = family_for(text, font);
    let font = family.as_deref();
    let style = buzz_text::FontStyle::REGULAR;
    let align = buzz_text::TextAlign::Centre;
    match preset.unit() {
        Unit::Line => buzz_text::outline_styled(text, size, font, style, align)
            .map(|path| vec![TitleUnit { text: text.to_string(), path }])
            .unwrap_or_default(),
        Unit::Cluster => buzz_text::outline_clusters(text, size, font, style, align)
            .into_iter()
            .filter(|c| !c.is_space(text) && !c.path.elements().is_empty())
            .map(|c| TitleUnit { text: c.text(text).to_string(), path: c.path })
            .collect(),
        Unit::Word => {
            let mut words: Vec<TitleUnit> = Vec::new();
            let mut open = false;
            for c in buzz_text::outline_clusters(text, size, font, style, align) {
                if c.is_space(text) {
                    open = false;
                    continue;
                }
                match words.last_mut() {
                    Some(word) if open => {
                        word.text.push_str(c.text(text));
                        word.path.extend(c.path.elements().iter().copied());
                    }
                    _ => {
                        words.push(TitleUnit { text: c.text(text).to_string(), path: c.path });
                        open = true;
                    }
                }
            }
            words.retain(|w| !w.path.elements().is_empty());
            words
        }
    }
}

/// Frames the in takes for `units` units.
pub fn in_frames(preset: TitlePreset, units: usize) -> u32 {
    let n = units.max(1) as u32;
    match preset {
        TitlePreset::Typewriter => 2 * n,
        TitlePreset::PopWords => 4 * (n - 1) + 6,
        TitlePreset::FadeUp => 12,
        TitlePreset::SlideIn => 14,
        TitlePreset::Stamp => 6 * n,
    }
}

/// A unit's state at one key.
#[derive(Debug, Clone, Copy)]
struct State {
    offset: Vec2,
    scale: f64,
    alpha: f32,
}

const REST: State = State { offset: Vec2::ZERO, scale: 1.0, alpha: 1.0 };

/// One key on a unit's layer: its state, and the tween to the next key
/// (`None` = no tween, hold).
type Keys = BTreeMap<u32, (State, Option<Easing>)>;

const EASE_OUT: Easing = Easing::Strength(100.0);
const EASE_IN: Easing = Easing::Strength(-100.0);
/// easeOutBack: ends ~10 % past the mark and settles back.
const OVERSHOOT: Easing = Easing::CubicBezier { x1: 0.34, y1: 1.56, x2: 0.64, y2: 1.0 };

/// **Add a title card** to `scene`: a card symbol (and one Graphic symbol per
/// unit) in the library under `Titles`, placed on a new top stage layer that
/// ignores the lights. The caller wraps this in one undo step.
pub fn add_title_card(scene: &mut Scene, spec: &TitleSpec) -> Result<TitleCard, TitleError> {
    let text = spec.text.trim_matches(|c: char| c == ' ' || c == '\t');
    if text.trim().is_empty() {
        return Err(TitleError::NoText);
    }
    if !(spec.size.is_finite() && spec.size > 0.0) {
        return Err(TitleError::BadSize);
    }
    let family = family_for(text, spec.font.as_deref());
    let pieces = units(text, spec.size, family.as_deref(), spec.preset);
    if pieces.is_empty() {
        return Err(TitleError::NoFont);
    }

    let stage = scene.stage().stage_rect();
    let fps = scene.stage().frame_rate.max(1.0);
    let n = pieces.len();
    let in_len = in_frames(spec.preset, n);
    let hold = match spec.total_frames {
        Some(total) => total.saturating_sub(in_len + OUT_FRAMES),
        None => (spec.hold_seconds.max(0.0) * fps).round() as u32,
    };
    let total = in_len + hold + OUT_FRAMES;
    let out_start = total - OUT_FRAMES;
    let last = total - 1;

    // Where the block sits: its visual centre on the anchor.
    let block = pieces
        .iter()
        .map(|u| u.path.bounding_box())
        .reduce(|a, b| a.union(b))
        .unwrap_or(Rect::ZERO);
    let anchor = match spec.position {
        TitlePosition::Centre => stage.center(),
        TitlePosition::LowerThird => Point::new(stage.center().x, stage.y0 + stage.height() * 0.78),
        TitlePosition::Top => Point::new(stage.center().x, stage.y0 + stage.height() * 0.16),
    };

    // Keys per unit, by preset.
    let rise = Vec2::new(0.0, stage.height() * 0.04);
    let off_left = Vec2::new(-(anchor.x - stage.x0 + block.width() / 2.0 + spec.size), 0.0);
    let gone = State { alpha: 0.0, ..REST };
    let unit_keys: Vec<Keys> = (0..n as u32)
        .map(|i| {
            let mut keys = Keys::new();
            match spec.preset {
                TitlePreset::Typewriter => {
                    keys.insert(2 * i, (REST, None));
                }
                TitlePreset::PopWords => {
                    let s = 4 * i;
                    keys.insert(s, (State { scale: 0.001, ..REST }, Some(EASE_OUT)));
                    keys.insert(s + 4, (State { scale: 1.15, ..REST }, Some(Easing::Linear)));
                    keys.insert(s + 6, (REST, None));
                }
                TitlePreset::FadeUp => {
                    keys.insert(0, (State { offset: rise, alpha: 0.0, ..REST }, Some(EASE_OUT)));
                    keys.insert(in_len, (REST, None));
                }
                TitlePreset::SlideIn => {
                    keys.insert(0, (State { offset: off_left, ..REST }, Some(OVERSHOOT)));
                    keys.insert(in_len, (REST, None));
                }
                TitlePreset::Stamp => {
                    let s = 6 * i;
                    keys.insert(s, (State { scale: 1.6, ..REST }, Some(EASE_IN)));
                    keys.insert(s + 4, (REST, None));
                }
            }
            // The out: the reverse for the two whole-line presets, a fade for
            // the rest. The last frame is always transparent.
            let (out_ease, end) = match spec.preset {
                TitlePreset::FadeUp => (EASE_IN, State { offset: rise, alpha: 0.0, ..REST }),
                TitlePreset::SlideIn => (EASE_IN, State { offset: off_left, alpha: 0.0, ..REST }),
                _ => (Easing::Linear, gone),
            };
            keys.insert(out_start, (REST, Some(out_ease)));
            keys.insert(last, (end, None));
            keys
        })
        .collect();

    // The library: one Graphic symbol per unit, its art centred on its own
    // origin so it scales about its middle.
    let short: String = text.chars().take(24).collect();
    let card_name = format!("Title: {short}");
    let card = scene.add_symbol(card_name.clone(), SymbolKind::Graphic, Some("Titles"));
    let mut layers: Vec<Layer> = Vec::new();
    for (i, (piece, keys)) in pieces.iter().zip(&unit_keys).enumerate() {
        let bounds = piece.path.bounding_box();
        let centre = bounds.center();
        let art = Affine::translate(-centre.to_vec2()) * piece.path.clone();
        let unit_symbol = scene.add_symbol(
            format!("{card_name} {:02} {}", i + 1, piece.text),
            SymbolKind::Graphic,
            Some("Titles/Units"),
        );
        let shape_id = scene.next_object_id();
        fill_symbol(scene, unit_symbol, Object::shape(shape_id, ShapeData::filled(art, spec.color)));
        let instance_id = scene.next_object_id();
        let at = anchor + (centre - block.center());
        let mut layer = Layer::normal(
            LayerId(scene.next_object_id().0),
            format!("{:02} {}", i + 1, piece.text),
        );
        layer.frames = keyed(keys, total, |state| {
            unit_object(instance_id, unit_symbol, at, state)
        });
        layers.push(layer);
    }

    // The plate, under the text: fades in over the first 8 frames (or the
    // whole in, if shorter) and out with the text.
    if let Some(plate) = spec.plate {
        let pad = Vec2::new(spec.size * 0.45, spec.size * 0.3);
        let rect = Rect::from_center_size(Point::ZERO, block.size() + kurbo::Size::new(pad.x * 2.0, pad.y * 2.0));
        let path = kurbo::RoundedRect::from_rect(rect, spec.size * 0.25).to_path(0.1);
        let [r, g, b, _] = plate.color.to_rgba8().to_u8_array();
        let opacity = plate.opacity.clamp(0.0, 1.0);
        let fill = Color::from_rgba8(r, g, b, (opacity * 255.0).round() as u8);
        let plate_symbol = scene.add_symbol(format!("{card_name} plate"), SymbolKind::Graphic, Some("Titles/Units"));
        let shape_id = scene.next_object_id();
        fill_symbol(scene, plate_symbol, Object::shape(shape_id, ShapeData::filled(path, fill)));
        let instance_id = scene.next_object_id();
        // A whole-line card carries its plate with it (it slides or rises
        // with the words); a card that builds unit by unit fades it.
        let keys = if spec.preset.unit() == Unit::Line {
            unit_keys[0].clone()
        } else {
            let mut keys = Keys::new();
            keys.insert(0, (gone, Some(EASE_OUT)));
            keys.insert(in_len.clamp(1, 8), (REST, None));
            keys.insert(out_start.max(in_len.clamp(1, 8)), (REST, Some(Easing::Linear)));
            keys.insert(last, (gone, None));
            keys
        };
        let mut layer = Layer::normal(LayerId(scene.next_object_id().0), "Plate");
        layer.frames = keyed(&keys, total, |state| {
            unit_object(instance_id, plate_symbol, anchor, state)
        });
        layers.push(layer);
    }

    scene.library_mut().update(card, |s| {
        let default: Vec<LayerId> = s.layers.iter().map(|l| l.id).collect();
        for id in default {
            s.layers.remove(id);
        }
        // Unit 1 on top, the plate at the bottom.
        for layer in layers.into_iter().rev() {
            s.layers.push_front(layer);
        }
    });

    // The stage: a new top layer that ignores the lights, holding the card
    // from `start` for `total` frames.
    let layer = scene.add_stage_layer(card_name.clone(), LayerKind::Normal);
    let card_instance = {
        let mut o = Object::instance_of(scene.next_object_id(), card);
        o.name = Some(card_name);
        if let ObjectKind::Instance(i) = &mut o.kind {
            i.loop_mode = LoopMode::PlayOnce;
        }
        Arc::new(o)
    };
    let start = spec.start;
    scene.update_layer(layer, |l| {
        l.unlit = true;
        let mut keyframes = Vec::new();
        if start > 0 {
            keyframes.push(Keyframe::new(0));
        }
        let mut on = Keyframe::new(start);
        on.objects = Arc::new(vec![card_instance]);
        keyframes.push(on);
        keyframes.push(Keyframe::new(start + total));
        l.frames = LayerTimeline::from_parts(keyframes, start + total + 1);
    });

    Ok(TitleCard { layer, symbol: card, start, frames: total, units: n })
}

/// Put `object` on the first layer of a fresh symbol.
fn fill_symbol(scene: &mut Scene, symbol: SymbolId, object: Object) {
    scene.library_mut().update(symbol, |s| {
        let first = s.layers.iter().next().map(|l| l.id);
        if let Some(id) = first {
            s.layers.update(id, |l| {
                l.frames.set_objects(0, vec![Arc::new(object)]);
            });
        }
    });
}

/// A unit's instance at one key.
fn unit_object(id: buzz_scene::ObjectId, symbol: SymbolId, at: Point, state: State) -> Object {
    let mut o = Object::instance_of(id, symbol)
        .with_transform(Affine::translate(at.to_vec2() + state.offset) * Affine::scale(state.scale));
    if let ObjectKind::Instance(i) = &mut o.kind {
        i.loop_mode = LoopMode::SingleFrame;
        if state.alpha < 1.0 {
            i.color = ColorTransform::alpha(state.alpha);
        }
    }
    o
}

/// A unit layer's timeline from its keys: blank until the first key, then one
/// keyframe per key, classic-tweened where the key asks for it.
fn keyed(keys: &Keys, total: u32, object: impl Fn(State) -> Object) -> LayerTimeline {
    let mut keyframes = Vec::new();
    if keys.keys().next().copied().unwrap_or(0) > 0 {
        keyframes.push(Keyframe::new(0));
    }
    for (&frame, &(state, ease)) in keys {
        let mut k = Keyframe::new(frame);
        k.objects = Arc::new(vec![Arc::new(object(state))]);
        if let Some(easing) = ease {
            k.tween = Tween { easing, ..Tween::classic() };
        }
        keyframes.push(k);
    }
    LayerTimeline::from_parts(keyframes, total)
}
