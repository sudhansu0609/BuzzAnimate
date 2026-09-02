//! Serialisable mirror of [`buzz_scene::Scene`].
//!
//! # Why a separate set of types
//!
//! Deriving `Serialize` directly on the runtime model would weld the file
//! format to internal struct layout: renaming a field or reordering an enum
//! would silently change the format and break every saved document. These DTOs
//! are a deliberate seam. The runtime model is free to evolve; the format
//! changes only when [`FORMAT_VERSION`] says so.
//!
//! # Two representation choices worth stating
//!
//! * **Paths are SVG strings.** Serialising `PathEl` as JSON enums is verbose —
//!   a 200-segment path becomes a wall of objects. `to_svg`/`from_svg` is
//!   compact, diff-friendly, and matches what SVG and XFL already do. It is
//!   also *lossless*: kurbo formats coordinates with Rust's `Display` for
//!   `f64`, which emits the shortest string that round-trips exactly. A test
//!   pins that down for extreme values.
//! * **Colours are `#RRGGBBAA` strings.** peniko can serialise `Color` itself,
//!   but that ties the format to peniko's internal representation across
//!   versions. Hex is stable, readable, and what every other vector format
//!   uses.

use std::sync::Arc;

use buzz_geom::{Affine, BezPath, FillMode, Size};
use buzz_scene::{
    ColorTransform, FillSpec, Gradient, GradientKind, GradientSpread, Layer, LayerHeight, LayerId,
    LayerKind, LoopMode, Object, ObjectId, ObjectKind, Paint, PaintBlend, Scene, ShapeData,
    StageProperties, StrokeSpec, Symbol, SymbolId, SymbolInstance, SymbolKind, Tween,
};
use peniko::Color;
use serde::{Deserialize, Serialize};

/// Bumped only for a breaking change to the on-disk layout.
///
/// * **1** — layers held a flat object list.
/// * **2** — layers hold keyframes, and the document has a camera track.
/// * **3** — a library of symbols, instance objects, and tweens on keyframes.
/// * **4** — shapes carry a paint blend, so build-up strokes survive a save.
/// * **5** — layers carry a depth, and the camera a focal distance.
/// * **6** — armatures and warped artwork: rigging, in Phase 7.
/// * **7** — sound: a library of clips, and sounds attached to keyframes.
/// * **8** — lights: a sun, a sky and lamps, and the shading they imply.
/// * **12** — a looping section: a range of frames the export repeats.
/// * **13** — named swatches, in folders: the document's palette.
/// * **14** — a transformation point per object.
/// * **15** — the inverse mask: a layer kind that hides what it covers.
/// * **16** — gradients: a fill or a stroke can be a ramp rather than a colour.
/// * **17** — bitmaps: a fill can be an image. The pictures live in `media/`
///   beside the sounds, one entry each, and a fill refers to one by id — so a
///   photograph placed forty times is stored once and `document.json` stays a
///   text file. A document with no bitmap is written exactly as version 16
///   wrote it.
///
/// Version 16 makes `color` optional on a fill and a stroke, and adds a
/// `gradient` beside it, of which exactly one is ever written. Older files
/// always carry `color` and never carry `gradient`, which is exactly what a
/// solid paint deserialises to — so every one of them still loads, and a
/// document that uses no gradient is written byte-identically to what version
/// 15 wrote.
///
/// Version 15 is a new *value*, not a new field: a document that has no
/// inverse mask is byte-identical to the one version 14 wrote. The bump is
/// still right, because a version 14 build reading a file that uses one would
/// fail on the unknown layer kind, and the version number is how it says so.
///
/// Every older version still loads. Version 1's flat list becomes a single
/// keyframe at frame 0, which is exactly what it meant; version 2 simply has
/// no library and no tweens, and both default to empty. Keeping those paths is
/// cheap and it exercises the version check for real rather than in theory.
///
/// * **20** — the compositor: the stage carries a `PostSettings` (bloom, grade,
///   vignette, grain). Absent in every older file and in any document with the
///   look left at its default, so a document with no effects is written exactly
///   as version 19 wrote it.
/// * **21** — 2.5D: lights carry a `LightTrack` so a sun can swing through a
///   shot; the stage carries `sort_by_depth`; the camera carries a keyable
///   aperture; and the camera carries named angles. Every one is absent in older
///   files and in any document that does not use it, so a document that keyframes
///   nothing is written exactly as version 20 wrote it.
/// * **22** — a lamp carries a `glow`: how much of its light is drawn as a pool
///   over the frame, which is what makes a lamp read as a lamp rather than as a
///   flat tint. Absent in older files, where it reads as full strength — the
///   honest reading of a lamp saved when there was no other kind.
/// * **23** — the `gloom`: a wall of darkness with a bearing, a throw and a
///   width, which is a new *kind* of light rather than a new field on the old
///   ones. It reuses `position` for where the wall stands and `azimuth` for
///   which way it faces, because those are the same two questions a lamp and a
///   sun already answer, and adds `throw` and `width`. A version 22 build
///   reading a file that uses one would load it as a sky — the fallback every
///   unknown kind takes — so the bump is how the file says not to.
/// * **24** — a light carries a `flicker`: how much it gutters, which is what
///   turns a lamp into a fire. Absent in older files and in every steady light,
///   so a document with no fire in it is written exactly as version 23 wrote it.
/// * **25** — a light carries a `rim`: how brightly it glows the outside edge of
///   the artwork it reaches. Absent in older files and in every light that does
///   not rim, which is the default, so a document that never asks for one is
///   written exactly as version 24 wrote it.
/// * **26** — an armature carries the name of the rig `pattern` it was
///   assembled from, so the Rigging panel can show a character's slots again a
///   week later and take a redrawn arm back into the one it belongs in. Absent
///   in older files and in every rig built a bone at a time with the Bone tool,
///   which is what a rig from version 25 is — those load with no pattern and
///   are written exactly as version 25 wrote them.
/// * **27** — an object carries a stack of live `modifiers` (a spring for
///   follow-through, a wiggle for idle motion), evaluated at draw time rather
///   than baked to keyframes. Absent in older files and in every object without
///   one, which is almost all of them, so an unchanged document is written
///   exactly as version 26 wrote it.
/// * **28** — a text object carries the `text` it was typed as (the string and
///   its size), so the words stay editable; the glyph outlines themselves are an
///   ordinary shape path. Absent in older files and in every non-text object, so
///   an unchanged document is written exactly as version 27 wrote it.
/// * **29** — an object may carry a `reverse` drawing (a whole nested object),
///   shown when it is turned to face away — a real turnaround rather than the
///   front mirrored. Absent in older files and in every object without a back,
///   so an unchanged document is written exactly as version 28 wrote it.
/// * **30** — a text object records the `font` family it was rendered with, so a
///   Hindi or calligraphy face survives a round-trip. Absent in older files and
///   in default-font text, so an unchanged document is written exactly as
///   version 29 wrote it.
/// * **31** — an image fill may `tile` (repeat seamlessly across its shape),
///   which is how procedural and bundled textures are drawn. Absent (and off)
///   for imported photos, so an unchanged document is written exactly as version
///   30 wrote it.
/// * **32** — a new `gradientmap` filter kind (a duotone recolour by
///   brightness), reusing the shadow/highlight colour keys. Files without one
///   are unchanged from version 31.
/// * **33** — the compositor gains posterise, halftone and hatching passes,
///   written flat on the post settings and defaulted off, so files without them
///   are unchanged from version 32.
pub const FORMAT_VERSION: u32 = 33;

/// Anything that can go wrong converting to or from the document model.
#[derive(Debug, thiserror::Error)]
pub enum SerialError {
    #[error("unsupported document version {found}; this build reads up to {supported}")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("could not parse path data: {0}")]
    BadPath(String),
    #[error("could not parse colour {0:?}; expected #RRGGBB or #RRGGBBAA")]
    BadColor(String),
    /// A value this build does not recognise, in a file whose version says it
    /// should. Refused rather than dropped: silently losing a filter would be
    /// worse than saying so.
    #[error("{0}")]
    Unsupported(String),
}

// ---------------------------------------------------------------------------
// Colour
// ---------------------------------------------------------------------------

fn color_to_hex(c: Color) -> String {
    let [r, g, b, a] = c.to_rgba8().to_u8_array();
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

fn color_from_hex(s: &str) -> Result<Color, SerialError> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    let byte = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| SerialError::BadColor(s.to_string()))
    };
    match hex.len() {
        6 => Ok(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, 255)),
        8 => Ok(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => Err(SerialError::BadColor(s.to_string())),
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentDto {
    pub format_version: u32,
    pub stage: StageDto,
    /// Front to back, as in the timeline.
    pub layers: Vec<LayerDto>,
    /// Highest id in use, so the allocator can resume safely.
    ///
    /// Symbols, layers and objects all draw from one allocator, so one figure
    /// covers all three — but it must be taken across the library as well as
    /// the stage, or a reopened document could hand out a symbol's id again.
    pub max_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraDto>,
    /// Version 3. Absent in older files, which had no symbols.
    #[serde(default, skip_serializing_if = "LibraryDto::is_empty")]
    pub library: LibraryDto,
    /// Imported sounds. Version 7. The audio itself lives in `media/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sounds: Vec<SoundAssetDto>,
    /// The lights. Version 8. Absent in every older file, and absent again in
    /// any document that has none — which is most of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lights: Option<LightRigDto>,
    /// The looping section. Version 12. Absent in older files and in any
    /// document that does not loop, which is most of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub looping: Option<LoopDto>,
    /// The palette. Version 13. Absent in older files, which get the default
    /// palette on the way in — which is what they effectively had.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swatches: Vec<SwatchDto>,
    /// Palette folders, including empty ones. Version 13.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swatch_folders: Vec<String>,
    /// Imported bitmaps. Version 17. The files live in `media/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageAssetDto>,
}

/// A named colour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwatchDto {
    pub id: u64,
    pub name: String,
    /// `#RRGGBB` or `#RRGGBBAA`, as every other colour in this format.
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// A range of frames that repeats, in playback and in the finished film.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopDto {
    pub start: u32,
    pub end: u32,
    pub repeats: u32,
}

/// The document's lights.
///
/// Written as its own small DTO rather than by deriving `Serialize` on the
/// runtime type: `LightRig` is free to gain fields without the format moving
/// underneath it, which is the whole reason this layer exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightRigDto {
    pub enabled: bool,
    pub base: String,
    pub modelling: f32,
    pub lights: Vec<LightDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightDto {
    pub id: u64,
    pub name: String,
    /// "sun", "sky" or "lamp".
    pub kind: String,
    pub color: String,
    pub intensity: f32,
    pub enabled: bool,
    pub shadows: bool,
    pub shadow_strength: f32,
    pub standing_height: f64,
    pub softness: f64,
    /// How much of a lamp's light is drawn as a pool. Version 22. Absent in
    /// older files, which is read as a lamp at full strength — that is what a
    /// lamp saved before the pool existed was meant to be.
    #[serde(default = "full_glow")]
    pub glow: f32,
    /// How much this light gutters. Version 24. Absent in older files and in
    /// every steady light, which is nearly all of them.
    #[serde(default, skip_serializing_if = "is_steady")]
    pub flicker: f32,
    /// How brightly this light rims what it reaches. Version 25. Absent in
    /// older files and in every light that does not rim, which is the default.
    #[serde(default, skip_serializing_if = "is_unrimmed")]
    pub rim: f32,
    /// Sun: the compass bearing and how high it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation: Option<f64>,
    /// Sky: the colour at the horizon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon: Option<String>,
    /// Lamp: where it is, how high, and how far it reaches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// Gloom: how far the darkness is thrown and how wide the wall is.
    /// Version 23. It stands at `position` and faces along `azimuth`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throw: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// The light's animation. Version 21. Absent in older files and in any
    /// static light, which is nearly all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<LightTrackDto>,
}

/// A light's keyframes. Version 21.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightTrackDto {
    pub enabled: bool,
    pub keys: Vec<LightKeyDto>,
}

/// One light keyframe. Carries the same flat kind fields as [`LightDto`], for
/// the same reason: the format must not be welded to a Rust enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightKeyDto {
    pub frame: u32,
    pub color: String,
    pub intensity: f32,
    pub softness: f64,
    pub standing_height: f64,
    pub shadow_strength: f32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throw: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
}

/// The flat serialised form of a [`buzz_scene::LightKind`]: a kind string plus
/// whichever numbers that kind needs. Shared by [`LightDto`] and [`LightKeyDto`]
/// so the two can never disagree on how a kind is written.
struct FlatKind {
    kind: String,
    azimuth: Option<f64>,
    elevation: Option<f64>,
    horizon: Option<String>,
    position: Option<[f64; 2]>,
    height: Option<f64>,
    radius: Option<f64>,
    throw: Option<f64>,
    width: Option<f64>,
}

/// A light that does not gutter writes no `flicker`, so an ordinary document is
/// byte-identical to what the version before this wrote.
fn is_steady(flicker: &f32) -> bool {
    *flicker == 0.0
}

/// A light that lays no rim writes no `rim`, for the same reason.
fn is_unrimmed(rim: &f32) -> bool {
    *rim == 0.0
}

fn kind_to_flat(kind: buzz_scene::LightKind) -> FlatKind {
    let mut flat = FlatKind {
        kind: kind.label().to_ascii_lowercase(),
        azimuth: None,
        elevation: None,
        horizon: None,
        position: None,
        height: None,
        radius: None,
        throw: None,
        width: None,
    };
    match kind {
        buzz_scene::LightKind::Sun { azimuth, elevation } => {
            flat.azimuth = Some(azimuth);
            flat.elevation = Some(elevation);
        }
        buzz_scene::LightKind::Sky { horizon } => {
            flat.horizon = Some(color_to_hex(horizon));
        }
        buzz_scene::LightKind::Lamp {
            position,
            height,
            radius,
        } => {
            flat.position = Some([position.x, position.y]);
            flat.height = Some(height);
            flat.radius = Some(radius);
        }
        // A gloom's bearing is written as an azimuth and its wall as a
        // position, because those are the same two questions a sun and a lamp
        // already answer and a second name for either would be a second thing
        // to keep in step.
        buzz_scene::LightKind::Gloom {
            edge,
            facing,
            throw,
            width,
        } => {
            flat.position = Some([edge.x, edge.y]);
            flat.azimuth = Some(facing);
            flat.throw = Some(throw);
            flat.width = Some(width);
        }
    }
    flat
}

#[allow(clippy::too_many_arguments)]
fn flat_to_kind(
    kind: &str,
    azimuth: Option<f64>,
    elevation: Option<f64>,
    horizon: Option<&str>,
    position: Option<[f64; 2]>,
    height: Option<f64>,
    radius: Option<f64>,
    throw: Option<f64>,
    width: Option<f64>,
) -> Result<buzz_scene::LightKind, SerialError> {
    Ok(match kind {
        "gloom" => buzz_scene::LightKind::Gloom {
            edge: position
                .map(|p| buzz_geom::Point::new(p[0], p[1]))
                .unwrap_or_default(),
            facing: azimuth.unwrap_or(0.0),
            throw: throw.unwrap_or(900.0),
            width: width.unwrap_or(2400.0),
        },
        "sun" => buzz_scene::LightKind::Sun {
            azimuth: azimuth.unwrap_or(-0.6),
            elevation: elevation.unwrap_or(0.9),
        },
        "lamp" => buzz_scene::LightKind::Lamp {
            position: position
                .map(|p| buzz_geom::Point::new(p[0], p[1]))
                .unwrap_or(buzz_geom::Point::new(275.0, 200.0)),
            height: height.unwrap_or(160.0),
            radius: radius.unwrap_or(320.0),
        },
        _ => buzz_scene::LightKind::Sky {
            horizon: horizon
                .map(color_from_hex)
                .transpose()?
                .unwrap_or(Color::from_rgb8(0x9A, 0x8C, 0x78)),
        },
    })
}

impl LightKeyDto {
    fn from_key(key: &buzz_scene::LightKey) -> Self {
        let flat = kind_to_flat(key.kind);
        Self {
            frame: key.frame,
            color: color_to_hex(key.color),
            intensity: key.intensity,
            softness: key.softness,
            standing_height: key.standing_height,
            shadow_strength: key.shadow_strength,
            kind: flat.kind,
            azimuth: flat.azimuth,
            elevation: flat.elevation,
            horizon: flat.horizon,
            position: flat.position,
            height: flat.height,
            radius: flat.radius,
            throw: flat.throw,
            width: flat.width,
        }
    }

    fn to_key(&self) -> Result<buzz_scene::LightKey, SerialError> {
        Ok(buzz_scene::LightKey {
            frame: self.frame,
            color: color_from_hex(&self.color)?,
            intensity: self.intensity,
            softness: self.softness,
            standing_height: self.standing_height,
            shadow_strength: self.shadow_strength,
            kind: flat_to_kind(
                &self.kind,
                self.azimuth,
                self.elevation,
                self.horizon.as_deref(),
                self.position,
                self.height,
                self.radius,
                self.throw,
                self.width,
            )?,
        })
    }
}

impl LightTrackDto {
    fn from_track(track: &buzz_scene::LightTrack) -> Self {
        Self {
            enabled: track.enabled,
            keys: track.keys().iter().map(LightKeyDto::from_key).collect(),
        }
    }

    fn to_track(&self) -> Result<buzz_scene::LightTrack, SerialError> {
        let mut track = buzz_scene::LightTrack::new();
        track.enabled = self.enabled;
        for key in &self.keys {
            track.set_key(key.to_key()?);
        }
        Ok(track)
    }
}

/// One filter on an object or a layer. Version 9.
///
/// Flat and hand-written, like [`LightDto`] and for the same reason: the file
/// format is a contract with every document ever saved, and deriving it
/// straight from the internal type would tie that contract to a Rust enum
/// somebody will one day want to rearrange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterDto {
    /// "blur", "dropshadow", "glow", "bevel" or "adjust".
    pub kind: String,
    #[serde(default = "yes")]
    pub enabled: bool,

    /// Blur radius across and down. Every filter but Adjust Color has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inner: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub knockout: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hide_object: bool,
    /// "low", "medium" or "high".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Bevel only: "inner", "outer" or "full".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bevel: Option<String>,

    /// Adjust Color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<f64>,
}

fn quality_name(q: buzz_scene::Quality) -> String {
    match q {
        buzz_scene::Quality::Low => "low",
        buzz_scene::Quality::Medium => "medium",
        buzz_scene::Quality::High => "high",
    }
    .to_string()
}

fn quality_from(name: &Option<String>) -> buzz_scene::Quality {
    match name.as_deref() {
        Some("low") => buzz_scene::Quality::Low,
        Some("high") => buzz_scene::Quality::High,
        // Anything unrecognised takes the default rather than refusing the
        // file: a quality setting is not worth losing a document over.
        _ => buzz_scene::Quality::Medium,
    }
}

fn blend_name(blend: buzz_scene::Blend) -> &'static str {
    use buzz_scene::Blend::*;
    match blend {
        Normal => "normal",
        Layer => "layer",
        Darken => "darken",
        Multiply => "multiply",
        Lighten => "lighten",
        Screen => "screen",
        Overlay => "overlay",
        HardLight => "hardlight",
        Add => "add",
        Difference => "difference",
    }
}

fn blend_from(name: &str) -> buzz_scene::Blend {
    use buzz_scene::Blend::*;
    match name {
        "layer" => Layer,
        "darken" => Darken,
        "multiply" => Multiply,
        "lighten" => Lighten,
        "screen" => Screen,
        "overlay" => Overlay,
        "hardlight" => HardLight,
        "add" => Add,
        "difference" => Difference,
        _ => Normal,
    }
}

fn is_normal_object_blend(blend: &buzz_scene::Blend) -> bool {
    *blend == buzz_scene::Blend::Normal
}

impl FilterDto {
    fn from_filter(filter: &buzz_scene::Filter) -> Self {
        use buzz_scene::FilterKind::*;
        let mut dto = Self {
            kind: String::new(),
            enabled: filter.enabled,
            x: None,
            y: None,
            strength: None,
            angle: None,
            distance: None,
            color: None,
            highlight: None,
            shadow: None,
            inner: false,
            knockout: false,
            hide_object: false,
            quality: None,
            bevel: None,
            brightness: None,
            contrast: None,
            saturation: None,
            hue: None,
        };

        match &filter.kind {
            Blur { x, y, quality } => {
                dto.kind = "blur".into();
                dto.x = Some(*x);
                dto.y = Some(*y);
                dto.quality = Some(quality_name(*quality));
            }
            DropShadow {
                x,
                y,
                strength,
                angle,
                distance,
                color,
                inner,
                knockout,
                hide_object,
                quality,
            } => {
                dto.kind = "dropshadow".into();
                dto.x = Some(*x);
                dto.y = Some(*y);
                dto.strength = Some(*strength);
                dto.angle = Some(*angle);
                dto.distance = Some(*distance);
                dto.color = Some(color_to_hex(*color));
                dto.inner = *inner;
                dto.knockout = *knockout;
                dto.hide_object = *hide_object;
                dto.quality = Some(quality_name(*quality));
            }
            Glow {
                x,
                y,
                strength,
                color,
                inner,
                knockout,
                quality,
            } => {
                dto.kind = "glow".into();
                dto.x = Some(*x);
                dto.y = Some(*y);
                dto.strength = Some(*strength);
                dto.color = Some(color_to_hex(*color));
                dto.inner = *inner;
                dto.knockout = *knockout;
                dto.quality = Some(quality_name(*quality));
            }
            Bevel {
                x,
                y,
                strength,
                angle,
                distance,
                highlight,
                shadow,
                kind,
                knockout,
                quality,
            } => {
                dto.kind = "bevel".into();
                dto.x = Some(*x);
                dto.y = Some(*y);
                dto.strength = Some(*strength);
                dto.angle = Some(*angle);
                dto.distance = Some(*distance);
                dto.highlight = Some(color_to_hex(*highlight));
                dto.shadow = Some(color_to_hex(*shadow));
                dto.knockout = *knockout;
                dto.quality = Some(quality_name(*quality));
                dto.bevel = Some(
                    match kind {
                        buzz_scene::BevelKind::Inner => "inner",
                        buzz_scene::BevelKind::Outer => "outer",
                        buzz_scene::BevelKind::Full => "full",
                    }
                    .to_string(),
                );
            }
            Adjust(adjust) => {
                dto.kind = "adjust".into();
                dto.brightness = Some(adjust.brightness);
                dto.contrast = Some(adjust.contrast);
                dto.saturation = Some(adjust.saturation);
                dto.hue = Some(adjust.hue);
            }
            GradientMap(map) => {
                // A duotone: the two ends reuse the shadow/highlight fields the
                // Bevel already carries, so no new keys are needed.
                dto.kind = "gradientmap".into();
                dto.shadow = Some(color_to_hex(map.shadow));
                dto.highlight = Some(color_to_hex(map.highlight));
            }
        }
        dto
    }

    fn to_filter(&self) -> Result<buzz_scene::Filter, SerialError> {
        use buzz_scene::FilterKind;

        let colour = |value: &Option<String>, fallback: Color| -> Result<Color, SerialError> {
            match value {
                Some(hex) => color_from_hex(hex),
                None => Ok(fallback),
            }
        };

        let kind = match self.kind.as_str() {
            "blur" => FilterKind::Blur {
                x: self.x.unwrap_or(5.0),
                y: self.y.unwrap_or(5.0),
                quality: quality_from(&self.quality),
            },
            "dropshadow" => FilterKind::DropShadow {
                x: self.x.unwrap_or(5.0),
                y: self.y.unwrap_or(5.0),
                strength: self.strength.unwrap_or(1.0),
                angle: self.angle.unwrap_or(std::f64::consts::FRAC_PI_4),
                distance: self.distance.unwrap_or(5.0),
                color: colour(&self.color, Color::BLACK)?,
                inner: self.inner,
                knockout: self.knockout,
                hide_object: self.hide_object,
                quality: quality_from(&self.quality),
            },
            "glow" => FilterKind::Glow {
                x: self.x.unwrap_or(5.0),
                y: self.y.unwrap_or(5.0),
                strength: self.strength.unwrap_or(1.0),
                color: colour(&self.color, Color::WHITE)?,
                inner: self.inner,
                knockout: self.knockout,
                quality: quality_from(&self.quality),
            },
            "bevel" => FilterKind::Bevel {
                x: self.x.unwrap_or(5.0),
                y: self.y.unwrap_or(5.0),
                strength: self.strength.unwrap_or(1.0),
                angle: self.angle.unwrap_or(std::f64::consts::FRAC_PI_4),
                distance: self.distance.unwrap_or(5.0),
                highlight: colour(&self.highlight, Color::WHITE)?,
                shadow: colour(&self.shadow, Color::BLACK)?,
                kind: match self.bevel.as_deref() {
                    Some("outer") => buzz_scene::BevelKind::Outer,
                    Some("full") => buzz_scene::BevelKind::Full,
                    _ => buzz_scene::BevelKind::Inner,
                },
                knockout: self.knockout,
                quality: quality_from(&self.quality),
            },
            "adjust" => FilterKind::Adjust(buzz_scene::ColorAdjust {
                brightness: self.brightness.unwrap_or_default(),
                contrast: self.contrast.unwrap_or_default(),
                saturation: self.saturation.unwrap_or_default(),
                hue: self.hue.unwrap_or_default(),
            }),
            "gradientmap" => FilterKind::GradientMap(buzz_scene::GradientMap {
                shadow: colour(&self.shadow, Color::from_rgb8(0x2B, 0x1D, 0x10))?,
                highlight: colour(&self.highlight, Color::from_rgb8(0xF2, 0xE6, 0xCF))?,
            }),
            other => {
                return Err(SerialError::Unsupported(format!(
                    "unknown filter kind {other:?}"
                )));
            }
        };

        Ok(buzz_scene::Filter {
            kind,
            enabled: self.enabled,
        })
    }
}

/// An object's facing in space. Version 11.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SpatialDto {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation_x: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation_y: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation_z: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub z: f64,
}

/// The document library: symbols plus the folder tree they sit in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryDto {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<SymbolDto>,
    /// Folder paths, including empty ones.
    ///
    /// Written separately from the symbols' own `folder` fields because
    /// Animate lets you create a folder before putting anything in it, and
    /// silently losing it on save would be surprising.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
}

impl LibraryDto {
    fn is_empty(&self) -> bool {
        self.symbols.is_empty() && self.folders.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolDto {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub kind: SymbolKind,
    /// Slash-separated library folder, or absent for the library root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Registration point — the origin an instance's transform is about.
    #[serde(default)]
    pub registration: [f64; 2],
    /// A symbol has its own timeline, with the same shape as the stage's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageDto {
    pub width: f64,
    pub height: f64,
    pub background: String,
    pub frame_rate: f64,
    /// The full-frame look. Version 20. Absent in older files and in any
    /// document whose effects are at their default, which is most of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<PostDto>,
    /// Draw layers by depth rather than the timeline. Version 21. Absent when
    /// off, which is every older file and most documents.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sort_by_depth: bool,
}

/// The compositor settings. Written flat, like [`LightDto`], so the runtime
/// [`buzz_scene::PostSettings`] can gain fields without moving the format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostDto {
    pub enabled: bool,
    pub bloom: BloomDto,
    pub grade: GradeDto,
    pub vignette: VignetteDto,
    pub grain: GrainDto,
    /// Posterise and halftone, flat and defaulted so older files (which have
    /// neither) load with both off. Version 33.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub posterise: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posterise_levels: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub halftone: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub halftone_scale: Option<f32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hatching: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hatching_scale: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomDto {
    pub enabled: bool,
    pub threshold: f32,
    pub intensity: f32,
    pub radius: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradeDto {
    pub enabled: bool,
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub lift: f32,
    pub gamma: f32,
    pub gain: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VignetteDto {
    pub enabled: bool,
    pub amount: f32,
    pub softness: f32,
    /// `#RRGGBB` or `#RRGGBBAA`, as every other colour in this format.
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrainDto {
    pub enabled: bool,
    pub amount: f32,
    pub size: f32,
}

impl PostDto {
    /// Written only when the look is not the default, so an unmodified document
    /// is byte-identical to what version 19 wrote.
    fn from_settings(p: &buzz_scene::PostSettings) -> Option<Self> {
        if *p == buzz_scene::PostSettings::default() {
            return None;
        }
        Some(Self {
            enabled: p.enabled,
            bloom: BloomDto {
                enabled: p.bloom.enabled,
                threshold: p.bloom.threshold,
                intensity: p.bloom.intensity,
                radius: p.bloom.radius,
            },
            grade: GradeDto {
                enabled: p.grade.enabled,
                exposure: p.grade.exposure,
                contrast: p.grade.contrast,
                saturation: p.grade.saturation,
                temperature: p.grade.temperature,
                tint: p.grade.tint,
                lift: p.grade.lift,
                gamma: p.grade.gamma,
                gain: p.grade.gain,
            },
            vignette: VignetteDto {
                enabled: p.vignette.enabled,
                amount: p.vignette.amount,
                softness: p.vignette.softness,
                color: color_to_hex(p.vignette.color),
            },
            grain: GrainDto {
                enabled: p.grain.enabled,
                amount: p.grain.amount,
                size: p.grain.size,
            },
            posterise: p.posterise.enabled,
            posterise_levels: Some(p.posterise.levels),
            halftone: p.halftone.enabled,
            halftone_scale: Some(p.halftone.scale),
            hatching: p.hatching.enabled,
            hatching_scale: Some(p.hatching.scale),
        })
    }

    fn to_settings(&self) -> Result<buzz_scene::PostSettings, SerialError> {
        Ok(buzz_scene::PostSettings {
            enabled: self.enabled,
            bloom: buzz_scene::BloomSettings {
                enabled: self.bloom.enabled,
                threshold: self.bloom.threshold,
                intensity: self.bloom.intensity,
                radius: self.bloom.radius,
            },
            grade: buzz_scene::GradeSettings {
                enabled: self.grade.enabled,
                exposure: self.grade.exposure,
                contrast: self.grade.contrast,
                saturation: self.grade.saturation,
                temperature: self.grade.temperature,
                tint: self.grade.tint,
                lift: self.grade.lift,
                gamma: self.grade.gamma,
                gain: self.grade.gain,
            },
            vignette: buzz_scene::VignetteSettings {
                enabled: self.vignette.enabled,
                amount: self.vignette.amount,
                softness: self.vignette.softness,
                color: color_from_hex(&self.vignette.color)?,
            },
            grain: buzz_scene::GrainSettings {
                enabled: self.grain.enabled,
                amount: self.grain.amount,
                size: self.grain.size,
            },
            posterise: buzz_scene::PosteriseSettings {
                enabled: self.posterise,
                levels: self.posterise_levels.unwrap_or(6),
            },
            halftone: buzz_scene::HalftoneSettings {
                enabled: self.halftone,
                scale: self.halftone_scale.unwrap_or(6.0),
            },
            hatching: buzz_scene::HatchingSettings {
                enabled: self.hatching,
                scale: self.hatching_scale.unwrap_or(6.0),
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerDto {
    pub id: u64,
    pub name: String,
    pub kind: LayerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    /// The layer this one follows — Animate's Layer Parenting. Version 9;
    /// absent means it follows nothing, which is every older document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follows: Option<u64>,
    /// The pose a rig parent was linked at. Version 10; absent on older
    /// documents, which fall back to their first keyframe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest_pose: Option<[f64; 6]>,
    pub visible: bool,
    pub locked: bool,
    #[serde(default)]
    pub outline: bool,
    /// Working transparency. Version 18; absent means fully solid, which is
    /// every layer of every older document.
    #[serde(default = "full", skip_serializing_if = "is_one")]
    pub alpha: f64,
    pub color: String,
    #[serde(default)]
    pub height: LayerHeight,
    /// Distance from the camera. Version 5; absent means the focal plane.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub depth: f64,
    #[serde(default)]
    pub collapsed: bool,
    /// Filters applied to the whole layer. Version 9.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FilterDto>,
    /// Frames the layer occupies. At least 1.
    #[serde(default = "one")]
    pub length: u32,
    /// Keyframes, sorted by start frame.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyframes: Vec<KeyframeDto>,
    /// Version 1's flat object list. Read, never written.
    #[serde(default, skip_serializing)]
    pub objects: Vec<ObjectDto>,
}

fn one() -> u32 {
    1
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

/// The default for a fraction whose absence means "all of it".
fn full() -> f64 {
    1.0
}

fn is_one(value: &f64) -> bool {
    *value == 1.0
}

/// An angle from a file, made safe. A corrupt one must not turn an object into
/// a NaN and take the whole frame with it.
fn sane_angle(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyframeDto {
    pub start: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<ObjectDto>,
    /// The tween running from here to the next keyframe. Version 3.
    ///
    /// Stored as the runtime type: `Tween` is a small closed value like
    /// `LayerKind`, already part of the format's vocabulary rather than an
    /// internal layout detail.
    #[serde(default, skip_serializing_if = "tween_is_absent")]
    pub tween: Tween,
    /// A sound starting on this keyframe. Version 7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<SoundDto>,
}

/// A sound placed on a keyframe.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SoundDto {
    pub sound: u64,
    #[serde(default)]
    pub sync: buzz_scene::SoundSync,
    pub volume: f32,
    #[serde(default = "one")]
    pub loops: u32,
}

/// An imported sound. The audio itself lives in the container's `media/`
/// directory rather than in this JSON: base64 in a document is four bytes on
/// disk for every three of audio, and unreadable in a diff either way.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundAssetDto {
    pub id: u64,
    pub name: String,
    /// File extension, which is also how the media entry is named.
    pub format: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Length in sample frames, so duration is known without decoding.
    pub length: u64,
}

fn tween_is_absent(tween: &Tween) -> bool {
    !tween.is_active()
}

fn is_normal_blend(blend: &PaintBlend) -> bool {
    *blend == PaintBlend::Normal
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraDto {
    pub enabled: bool,
    /// Distance to the depth-zero plane. Version 5; older files take the
    /// default, which leaves a document with no depth looking identical.
    #[serde(default = "default_focal_distance")]
    pub focal_distance: f64,
    /// Depth-of-field strength. Version 21; absent (0) is a pinhole camera.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub aperture: f64,
    /// The layer depth in focus. Version 21.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub focus_depth: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<CameraKeyDto>,
    /// Named camera angles — Wave 10b. Version 21. Absent for any document that
    /// saves none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub angles: Vec<NamedAngleDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedAngleDto {
    pub name: String,
    pub state: CameraKeyDto,
}

fn camera_key_to_dto(k: &buzz_scene::CameraKey) -> CameraKeyDto {
    CameraKeyDto {
        frame: k.frame,
        x: k.center.x,
        y: k.center.y,
        zoom: k.zoom,
        rotation: k.rotation,
        pitch: k.pitch,
        yaw: k.yaw,
    }
}

fn camera_key_from_dto(dto: &CameraKeyDto) -> buzz_scene::CameraKey {
    buzz_scene::CameraKey {
        frame: dto.frame,
        center: buzz_geom::Point::new(dto.x, dto.y),
        zoom: dto.zoom,
        rotation: dto.rotation,
        pitch: dto.pitch,
        yaw: dto.yaw,
    }
}

fn default_focal_distance() -> f64 {
    buzz_scene::camera_track::DEFAULT_FOCAL_DISTANCE
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CameraKeyDto {
    pub frame: u32,
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
    pub rotation: f64,
    /// Tilt up and down, in radians. Version 10; absent means looking straight
    /// at the stage, which is every camera written before this.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pitch: f64,
    /// Turn left and right, in radians. Version 10.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub yaw: f64,
}

/// One live modifier on an object — a spring or a wiggle. Version 27.
///
/// Flat and hand-written like [`FilterDto`], for the same contract reason: the
/// stored form must not be chained to the shape of an internal Rust enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifierDto {
    /// "wiggle" or "spring".
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amplitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stiffness: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damping: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coupling: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
}

impl ModifierDto {
    fn from_modifier(m: &buzz_scene::Modifier) -> Self {
        use buzz_scene::Modifier;
        // Every field starts absent; each arm fills only the ones it uses, so an
        // unused field is skipped rather than written as null.
        let base = Self {
            kind: String::new(),
            amplitude: None,
            frequency: None,
            root: None,
            stiffness: None,
            damping: None,
            coupling: None,
            x: None,
            y: None,
            amount: None,
        };
        match *m {
            Modifier::Wiggle {
                amplitude,
                frequency,
            } => Self {
                kind: "wiggle".to_string(),
                amplitude: Some(amplitude),
                frequency: Some(frequency),
                ..base
            },
            Modifier::Spring {
                root,
                stiffness,
                damping,
                coupling,
            } => Self {
                kind: "spring".to_string(),
                root: Some(root as u64),
                stiffness: Some(stiffness),
                damping: Some(damping),
                coupling: Some(coupling),
                ..base
            },
            Modifier::LookAt { x, y } => Self {
                kind: "lookat".to_string(),
                x: Some(x),
                y: Some(y),
                ..base
            },
            Modifier::AutoSquashStretch { amount } => Self {
                kind: "squashstretch".to_string(),
                amount: Some(amount),
                ..base
            },
        }
    }

    /// The model modifier, or `None` for a kind this version does not know —
    /// a modifier from a newer file is dropped rather than failing the load.
    fn to_modifier(&self) -> Option<buzz_scene::Modifier> {
        use buzz_scene::Modifier;
        match self.kind.as_str() {
            "wiggle" => Some(Modifier::Wiggle {
                amplitude: self.amplitude.unwrap_or(0.0),
                frequency: self.frequency.unwrap_or(1.0),
            }),
            "spring" => Some(Modifier::Spring {
                root: self.root.unwrap_or(0) as usize,
                stiffness: self.stiffness.unwrap_or(120.0),
                damping: self.damping.unwrap_or(12.0),
                coupling: self.coupling.unwrap_or(0.0),
            }),
            "lookat" => Some(Modifier::LookAt {
                x: self.x.unwrap_or(0.0),
                y: self.y.unwrap_or(0.0),
            }),
            "squashstretch" => Some(Modifier::AutoSquashStretch {
                amount: self.amount.unwrap_or(0.0),
            }),
            _ => None,
        }
    }
}

/// The source of a text object. Version 28. The rendered outlines are the
/// Shape's path (serialized like any shape); this keeps the words editable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDataDto {
    pub content: String,
    pub size: f64,
    /// The font family the text was rendered with. Version 30; absent (and
    /// meaning "system default") in older files and any default-font text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectDto {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Affine coefficients `[a, b, c, d, e, f]`.
    #[serde(default = "identity_coeffs")]
    pub transform: [f64; 6],
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub locked: bool,
    /// Filters on this object. Version 9.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FilterDto>,
    /// How it blends with what is behind it. Version 9.
    #[serde(default, skip_serializing_if = "str::is_empty")]
    pub blend: String,
    /// Which way it faces in space — Animate's 3D Rotation and 3D Translation.
    /// Version 11; absent means flat, which is every object written before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial: Option<SpatialDto>,
    /// The transformation point, in the object's own coordinates. Version 14;
    /// absent means the centre of the artwork, which is what every object
    /// written before it used and what an untouched one still uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<[f64; 2]>,
    /// Live modifiers on this object. Version 27; absent means none, which is
    /// every object written before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<ModifierDto>,
    /// The text this object was typed as, when it is text. Version 28.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextDataDto>,
    /// The back drawing shown when the object faces away. Version 29. `Box`d so
    /// the DTO is not infinitely sized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<Box<ObjectDto>>,
    pub kind: ObjectKindDto,
}

fn identity_coeffs() -> [f64; 6] {
    Affine::IDENTITY.as_coeffs()
}

fn yes() -> bool {
    true
}

/// The default for a lamp's glow in a file written before there was one.
fn full_glow() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ObjectKindDto {
    Shape {
        /// SVG path data.
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<FillDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<StrokeDto>,
        /// How the shape combines with the paint under it. Version 4.
        ///
        /// Defaulted and omitted when Normal, so every version-3 document
        /// still loads and every ordinary shape stays as compact as it was.
        #[serde(default, skip_serializing_if = "is_normal_blend")]
        blend: PaintBlend,
    },
    Group {
        children: Vec<ObjectDto>,
    },
    /// A placed instance of a library symbol. Version 3.
    Instance {
        symbol: u64,
        #[serde(default)]
        first_frame: u32,
        #[serde(default)]
        loop_mode: LoopMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<ColorTransformDto>,
    },
    /// Artwork rigged to a skeleton. Version 6.
    ///
    /// Only the *pose* is stored — the angles — never the deformed artwork it
    /// produces. A saved deformation would be a second copy of something the
    /// document can already work out, and the two would drift apart the first
    /// time a bone was edited without re-saving every frame.
    Armature {
        root: [f64; 2],
        bones: Vec<BoneDto>,
        parts: Vec<RigPartDto>,
        /// Named poses this character owns. Version 19.
        ///
        /// Defaulted and skipped when empty, so a rig with no poses saves
        /// exactly as it did before and a file from version 18 reads back
        /// with none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        poses: Vec<NamedPoseDto>,
        /// The rig pattern this skeleton was assembled from. Version 26.
        ///
        /// A name, not the pattern: see `ArmatureData::pattern` for why the
        /// bones remain the truth. Absent for a rig built with the Bone tool,
        /// which is every rig written before version 26.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
    },
    /// Artwork with warp handles on it. Version 6.
    Warp {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<FillDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<StrokeDto>,
        #[serde(default, skip_serializing_if = "is_normal_blend")]
        blend: PaintBlend,
        handles: Vec<HandleDto>,
        rigidity: f64,
    },
}

/// One bone of an armature.
///
/// Angles are radians relative to the parent, which is what makes a pose
/// portable: the file records how far the elbow is bent, not where the hand
/// happens to be.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoneDto {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    pub length: f64,
    pub rest_angle: f64,
    pub angle: f64,
    /// Joint rotation limits, `[min, max]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
}

/// Artwork attached to an armature, and how it follows the bones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedPoseDto {
    pub name: String,
    pub angles: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigPartDto {
    pub artwork: ObjectDto,
    /// Per point, the bones that move it: `[bone, weight]` pairs. Empty for a
    /// rigidly attached part.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weights: Vec<Vec<(usize, f64)>>,
    /// The single bone a rigid part rides on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rigid_bone: Option<usize>,
}

/// A warp handle: where it was placed, and where it has been dragged.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HandleDto {
    pub rest: [f64; 2],
    pub current: [f64; 2],
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// An instance's colour effect.
///
/// Written as two arrays rather than as Animate's Advanced-panel percentages
/// so the stored value is exactly what the renderer multiplies by; the panel's
/// percentages are a view of it, not the truth.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ColorTransformDto {
    /// Per-channel multiplier, RGBA.
    pub multiply: [f32; 4],
    /// Per-channel offset in 0..=1 units, RGBA.
    pub add: [f32; 4],
}

/// One colour at one place along a gradient's ramp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradientStopDto {
    pub offset: f64,
    /// `#RRGGBB` or `#RRGGBBAA`, as every other colour in this format.
    pub color: String,
}

/// A gradient, in the unit space the model defines, plus where it goes.
///
/// The transform is stored as the six affine coefficients rather than as a
/// centre, an angle and a size. It is what the model holds, what SWF and XFL
/// hold, and what Animate's Gradient Transform tool edits — decomposing it into
/// named parts on the way out and recomposing on the way in would lose a skewed
/// gradient and gain nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradientDto {
    /// "linear" or "radial".
    pub kind: String,
    pub stops: Vec<GradientStopDto>,
    /// Unit space to the object's own space: `[a, b, c, d, e, f]`.
    pub transform: [f64; 6],
    /// "pad", "reflect" or "repeat". Absent means "pad", which is Animate's
    /// default and the only one that cannot show a seam.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<String>,
    /// Radial only: where the hot spot sits along the unit x axis.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub focal: f64,
}

/// A fill. **Exactly one of `color` and `gradient` is written.**
///
/// `color` is an `Option` so that a gradient fill does not also carry a flat
/// colour that means nothing and could disagree with the ramp. Older files
/// always have it, so they still load: a missing `gradient` and a present
/// `color` is precisely what every file before version 16 contains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Version 16.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<GradientDto>,
    /// Version 17.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageFillDto>,
    #[serde(default)]
    pub rule: FillMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrokeDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Version 16.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<GradientDto>,
    /// Version 17.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageFillDto>,
    pub width: f64,
    #[serde(default)]
    pub hairline: bool,
}

// ---------------------------------------------------------------------------
// Paint <-> DTO
// ---------------------------------------------------------------------------

fn gradient_to_dto(g: &Gradient) -> GradientDto {
    GradientDto {
        kind: match g.kind {
            GradientKind::Linear => "linear",
            GradientKind::Radial => "radial",
        }
        .to_string(),
        stops: g
            .stops()
            .iter()
            .map(|s| GradientStopDto {
                offset: s.offset,
                color: color_to_hex(s.color),
            })
            .collect(),
        transform: g.transform.as_coeffs(),
        spread: match g.spread {
            GradientSpread::Pad => None,
            GradientSpread::Reflect => Some("reflect".to_string()),
            GradientSpread::Repeat => Some("repeat".to_string()),
        },
        focal: g.focal,
    }
}

fn gradient_from_dto(dto: &GradientDto) -> Result<Gradient, SerialError> {
    let kind = match dto.kind.as_str() {
        "linear" => GradientKind::Linear,
        "radial" => GradientKind::Radial,
        other => {
            return Err(SerialError::Unsupported(format!(
                "unknown gradient kind {other:?}"
            )));
        }
    };
    let spread = match dto.spread.as_deref() {
        None | Some("pad") => GradientSpread::Pad,
        Some("reflect") => GradientSpread::Reflect,
        Some("repeat") => GradientSpread::Repeat,
        Some(other) => {
            return Err(SerialError::Unsupported(format!(
                "unknown gradient spread {other:?}"
            )));
        }
    };
    let stops = dto
        .stops
        .iter()
        .map(|s| {
            Ok(buzz_scene::GradientStop {
                offset: s.offset,
                color: color_from_hex(&s.color)?,
            })
        })
        .collect::<Result<Vec<_>, SerialError>>()?;

    let mut g = Gradient::new(kind, stops);
    g.transform = Affine::new(dto.transform);
    g.spread = spread;
    g.focal = dto.focal;
    Ok(g)
}

/// A bitmap fill: which image, and where it sits.
///
/// **The pixels are not here.** The image is written once into the container's
/// `media/` directory, byte for byte as it was imported, and referred to by id
/// — the same arrangement sounds have used since version 7, and for the same
/// reason: a document with one photograph placed forty times must not hold
/// forty copies of it, and `document.json` must stay a text file a person can
/// read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageFillDto {
    pub image: u64,
    /// Unit square to the object's own space: `[a, b, c, d, e, f]`.
    pub transform: [f64; 6],
    /// Absent means smoothed, which is what a photograph wants and what the
    /// overwhelming majority of images are.
    #[serde(default = "smoothing_on", skip_serializing_if = "is_smoothing_on")]
    pub smooth: bool,
    /// Repeat the image across the shape (a seamless texture) rather than
    /// stretching one copy. Version 31; absent (and off) for imported photos.
    #[serde(default, skip_serializing_if = "is_false")]
    pub tile: bool,
}

fn smoothing_on() -> bool {
    true
}

fn is_smoothing_on(v: &bool) -> bool {
    *v
}

/// An imported bitmap. The file itself lives in `media/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAssetDto {
    pub id: u64,
    pub name: String,
    /// File extension, which is also how the media entry is named.
    pub format: String,
    pub width: u32,
    pub height: u32,
}

/// Write a paint out as the fields the DTOs carry.
fn paint_to_dto(paint: &Paint) -> (Option<String>, Option<GradientDto>, Option<ImageFillDto>) {
    match paint {
        Paint::Solid(c) => (Some(color_to_hex(*c)), None, None),
        Paint::Gradient(g) => (None, Some(gradient_to_dto(g)), None),
        Paint::Image(i) => (
            None,
            None,
            Some(ImageFillDto {
                image: i.asset.id.0,
                transform: i.transform.as_coeffs(),
                smooth: i.smooth,
                tile: i.tile,
            }),
        ),
    }
}

/// Read a paint back.
///
/// A file with neither field is not an error: it is a shape whose colour is
/// missing, and refusing to open the whole document over one absent fill would
/// lose everything else in it. Black is what an unpainted fill has always
/// meant.
fn paint_from_dto(
    color: Option<&String>,
    gradient: Option<&GradientDto>,
    image: Option<&ImageFillDto>,
    images: &buzz_scene::ImageLibrary,
) -> Result<Paint, SerialError> {
    // **A bitmap whose file is missing falls back to a colour rather than
    // refusing the document.** A `.buzz` that lost its `media/` entry — copied
    // out of a zip by hand, or truncated — should still open with everything
    // else intact; the shape is there, its outline is there, and only the
    // picture inside it is gone.
    if let Some(fill) = image {
        match images.get(buzz_scene::ImageId(fill.image)) {
            Some(asset) => {
                return Ok(Paint::Image(Box::new(buzz_scene::ImageFill {
                    asset: std::sync::Arc::clone(asset),
                    transform: Affine::new(fill.transform),
                    smooth: fill.smooth,
                    tile: fill.tile,
                })));
            }
            None => return Ok(Paint::Solid(Color::from_rgb8(0x80, 0x80, 0x80))),
        }
    }
    match (gradient, color) {
        (Some(g), _) => Ok(Paint::Gradient(std::sync::Arc::new(gradient_from_dto(g)?))),
        (None, Some(c)) => Ok(Paint::Solid(color_from_hex(c)?)),
        (None, None) => Ok(Paint::Solid(Color::BLACK)),
    }
}

fn fill_to_dto(f: &FillSpec) -> FillDto {
    let (color, gradient, image) = paint_to_dto(&f.paint);
    FillDto {
        color,
        gradient,
        image,
        rule: f.rule,
    }
}

fn fill_from_dto(f: &FillDto, images: &buzz_scene::ImageLibrary) -> Result<FillSpec, SerialError> {
    Ok(FillSpec {
        paint: paint_from_dto(
            f.color.as_ref(),
            f.gradient.as_ref(),
            f.image.as_ref(),
            images,
        )?,
        rule: f.rule,
    })
}

fn stroke_to_dto(s: &StrokeSpec) -> StrokeDto {
    let (color, gradient, image) = paint_to_dto(&s.paint);
    StrokeDto {
        color,
        gradient,
        image,
        width: s.width,
        hairline: s.hairline,
    }
}

fn stroke_from_dto(
    s: &StrokeDto,
    images: &buzz_scene::ImageLibrary,
) -> Result<StrokeSpec, SerialError> {
    Ok(StrokeSpec {
        paint: paint_from_dto(
            s.color.as_ref(),
            s.gradient.as_ref(),
            s.image.as_ref(),
            images,
        )?,
        width: s.width,
        hairline: s.hairline,
    })
}

// ---------------------------------------------------------------------------
// Scene -> DTO
// ---------------------------------------------------------------------------

impl LayerDto {
    fn from_layer(layer: &Layer, max_id: &mut u64) -> Self {
        *max_id = (*max_id).max(layer.id.0);
        Self {
            id: layer.id.0,
            name: layer.name.clone(),
            kind: layer.kind,
            parent: layer.parent.map(|p| p.0),
            follows: layer.follows.map(|p| p.0),
            rest_pose: layer.rest_pose.map(|m| m.as_coeffs()),
            visible: layer.visible,
            locked: layer.locked,
            outline: layer.outline,
            alpha: layer.alpha,
            color: color_to_hex(layer.color),
            height: layer.height,
            depth: layer.depth,
            collapsed: layer.collapsed,
            filters: layer.filters.iter().map(FilterDto::from_filter).collect(),
            length: layer.frames.length(),
            keyframes: layer
                .frames
                .keyframes()
                .iter()
                .map(|k| KeyframeDto {
                    start: k.start,
                    label: k.label.clone(),
                    objects: k
                        .objects
                        .iter()
                        .map(|o| ObjectDto::from_object(o, max_id))
                        .collect(),
                    tween: k.tween,
                    sound: k.sound.map(|s| SoundDto {
                        sound: s.sound.0,
                        sync: s.sync,
                        volume: s.volume,
                        loops: s.loops,
                    }),
                })
                .collect(),
            objects: Vec::new(),
        }
    }

    fn to_layer(&self, images: &buzz_scene::ImageLibrary) -> Result<Layer, SerialError> {
        let mut layer = Layer::new(LayerId(self.id), self.name.clone(), self.kind);
        layer.parent = self.parent.map(LayerId);
        layer.follows = self.follows.map(LayerId);
        layer.rest_pose = self.rest_pose.map(buzz_geom::Affine::new);
        layer.visible = self.visible;
        layer.locked = self.locked;
        layer.outline = self.outline;
        // Clamped on the way in: a document written by hand, or by a build
        // that meant something else by this number, must not make a layer
        // invisible or brighter than it can be.
        layer.alpha = if self.alpha.is_finite() {
            self.alpha.clamp(0.0, 1.0)
        } else {
            1.0
        };
        layer.color = color_from_hex(&self.color)?;
        layer.height = self.height;
        // A corrupt value must not put a layer behind the camera for good.
        layer.depth = if self.depth.is_finite() {
            self.depth
        } else {
            0.0
        };
        layer.collapsed = self.collapsed;
        for filter in &self.filters {
            layer.filters.push(filter.to_filter()?);
        }

        // Version 1 stored a flat object list, which meant exactly "one
        // keyframe at frame 0". Translate rather than reject.
        let source: Vec<KeyframeDto> = if self.keyframes.is_empty() && !self.objects.is_empty() {
            vec![KeyframeDto {
                start: 0,
                label: None,
                objects: self.objects.clone(),
                tween: Tween::default(),
                sound: None,
            }]
        } else {
            self.keyframes.clone()
        };

        let mut keyframes = Vec::with_capacity(source.len());
        for k in &source {
            let mut objects = Vec::with_capacity(k.objects.len());
            for object in &k.objects {
                objects.push(Arc::new(object.to_object(images)?));
            }
            keyframes.push(buzz_scene::Keyframe {
                start: k.start,
                objects: Arc::new(objects),
                label: k.label.clone(),
                tween: k.tween,
                sound: k.sound.map(|s| buzz_scene::SoundRef {
                    sound: buzz_scene::SoundId(s.sound),
                    sync: s.sync,
                    volume: s.volume,
                    loops: s.loops,
                }),
            });
        }
        layer.frames = buzz_scene::LayerTimeline::from_parts(keyframes, self.length.max(1));
        Ok(layer)
    }
}

impl SymbolDto {
    fn from_symbol(symbol: &Symbol, max_id: &mut u64) -> Self {
        *max_id = (*max_id).max(symbol.id.0);
        Self {
            id: symbol.id.0,
            name: symbol.name.clone(),
            kind: symbol.kind,
            folder: symbol.folder.clone(),
            registration: [symbol.registration.x, symbol.registration.y],
            layers: symbol
                .layers
                .iter()
                .map(|l| LayerDto::from_layer(l, max_id))
                .collect(),
        }
    }

    fn to_symbol(&self, images: &buzz_scene::ImageLibrary) -> Result<Symbol, SerialError> {
        let mut symbol = Symbol::new(SymbolId(self.id), self.name.clone(), self.kind);
        symbol.folder = self.folder.clone();
        symbol.registration = buzz_geom::Point::new(self.registration[0], self.registration[1]);
        for (index, dto) in self.layers.iter().enumerate() {
            symbol.layers.insert(index, dto.to_layer(images)?);
        }
        Ok(symbol)
    }
}

impl DocumentDto {
    pub fn from_scene(scene: &Scene) -> Self {
        let mut max_id = 0u64;
        // `stage_layers`, not `layers`: with a symbol open for editing the
        // latter is the symbol's timeline, and saving it as the document's
        // would quietly replace the main timeline with the symbol's contents.
        let layers: Vec<LayerDto> = scene
            .stage_layers()
            .iter()
            .map(|layer| LayerDto::from_layer(layer, &mut max_id))
            .collect();

        // Symbols share the id allocator with stage layers and objects, so
        // their ids have to raise `max_id` too.
        let library = LibraryDto {
            symbols: scene
                .library()
                .iter()
                .map(|s| SymbolDto::from_symbol(s, &mut max_id))
                .collect(),
            folders: scene.library().folders().cloned().collect(),
        };

        Self {
            format_version: FORMAT_VERSION,
            stage: StageDto {
                width: scene.stage().size.width,
                height: scene.stage().size.height,
                background: color_to_hex(scene.stage().background),
                frame_rate: scene.stage().frame_rate,
                post: PostDto::from_settings(&scene.stage().post),
                sort_by_depth: scene.stage().sort_by_depth,
            },
            layers,
            max_id,
            camera: {
                let cam = scene.camera();
                let has_content = !cam.is_empty()
                    || cam.enabled
                    || cam.aperture != 0.0
                    || !cam.angles.is_empty();
                has_content.then(|| CameraDto {
                    enabled: cam.enabled,
                    focal_distance: cam.focal_distance,
                    aperture: cam.aperture,
                    focus_depth: cam.focus_depth,
                    keys: cam.keys().iter().map(camera_key_to_dto).collect(),
                    angles: cam
                        .angles
                        .iter()
                        .map(|a| NamedAngleDto {
                            name: a.name.clone(),
                            state: camera_key_to_dto(&a.state),
                        })
                        .collect(),
                })
            },
            library,
            sounds: scene
                .sounds()
                .iter()
                .map(|sound| {
                    max_id = max_id.max(sound.id.0);
                    SoundAssetDto {
                        id: sound.id.0,
                        name: sound.name.clone(),
                        format: sound.format.clone(),
                        sample_rate: sound.sample_rate,
                        channels: sound.channels,
                        length: sound.length,
                    }
                })
                .collect(),
            swatches: scene
                .swatches()
                .iter()
                .map(|s| SwatchDto {
                    id: s.id.0,
                    name: s.name.clone(),
                    color: color_to_hex(s.color),
                    folder: s.folder.clone(),
                })
                .collect(),
            swatch_folders: scene.swatches().folders().cloned().collect(),
            images: scene
                .images()
                .iter()
                .map(|image| {
                    max_id = max_id.max(image.id.0);
                    ImageAssetDto {
                        id: image.id.0,
                        name: image.name.clone(),
                        format: image.format.clone(),
                        width: image.width,
                        height: image.height,
                    }
                })
                .collect(),
            looping: scene.looping().enabled.then(|| LoopDto {
                start: scene.looping().start,
                end: scene.looping().end,
                repeats: scene.looping().repeats,
            }),
            lights: (!scene.lights().lights.is_empty()).then(|| LightRigDto {
                enabled: scene.lights().enabled,
                base: color_to_hex(scene.lights().base),
                modelling: scene.lights().modelling,
                lights: scene
                    .lights()
                    .lights
                    .iter()
                    .map(|light| {
                        max_id = max_id.max(light.id.0);
                        let mut dto = LightDto {
                            id: light.id.0,
                            name: light.name.clone(),
                            kind: light.kind.label().to_ascii_lowercase(),
                            color: color_to_hex(light.color),
                            intensity: light.intensity,
                            enabled: light.enabled,
                            shadows: light.shadows,
                            shadow_strength: light.shadow_strength,
                            standing_height: light.standing_height,
                            softness: light.softness,
                            glow: light.glow,
                            flicker: light.flicker,
                            rim: light.rim,
                            azimuth: None,
                            elevation: None,
                            horizon: None,
                            position: None,
                            height: None,
                            radius: None,
                            throw: None,
                            width: None,
                            track: light
                                .track
                                .as_ref()
                                .filter(|t| !t.is_empty())
                                .map(LightTrackDto::from_track),
                        };
                        match light.kind {
                            buzz_scene::LightKind::Sun { azimuth, elevation } => {
                                dto.azimuth = Some(azimuth);
                                dto.elevation = Some(elevation);
                            }
                            buzz_scene::LightKind::Sky { horizon } => {
                                dto.horizon = Some(color_to_hex(horizon));
                            }
                            buzz_scene::LightKind::Lamp {
                                position,
                                height,
                                radius,
                            } => {
                                dto.position = Some([position.x, position.y]);
                                dto.height = Some(height);
                                dto.radius = Some(radius);
                            }
                            buzz_scene::LightKind::Gloom {
                                edge,
                                facing,
                                throw,
                                width,
                            } => {
                                dto.position = Some([edge.x, edge.y]);
                                dto.azimuth = Some(facing);
                                dto.throw = Some(throw);
                                dto.width = Some(width);
                            }
                        }
                        dto
                    })
                    .collect(),
            }),
        }
    }

    /// Rebuild a scene. Fails only on genuinely malformed data.
    /// Rebuild the document, with no bitmaps.
    ///
    /// The container supplies them — they live in `media/` — so anything that
    /// reads a `.buzz` goes through [`Self::to_scene_with_images`]. This is for
    /// callers that have only the JSON, chiefly the tests.
    pub fn to_scene(&self) -> Result<Scene, SerialError> {
        self.to_scene_with_images(buzz_scene::ImageLibrary::default())
    }

    /// Rebuild the document with its bitmaps already decoded.
    ///
    /// The library is built **before** any layer is converted, because a fill
    /// refers to an image by id and resolves against it as it is read — the
    /// same ordering the symbol library needs, and for the same reason.
    pub fn to_scene_with_images(
        &self,
        images: buzz_scene::ImageLibrary,
    ) -> Result<Scene, SerialError> {
        if self.format_version > FORMAT_VERSION {
            return Err(SerialError::UnsupportedVersion {
                found: self.format_version,
                supported: FORMAT_VERSION,
            });
        }

        let mut scene = Scene::empty();
        // Installed before anything is converted, so a fill can resolve
        // against it as the layers are read.
        *scene.images_mut() = images.clone();
        let images = &images;
        *scene.stage_mut() = StageProperties {
            size: Size::new(self.stage.width, self.stage.height),
            background: color_from_hex(&self.stage.background)?,
            frame_rate: self.stage.frame_rate,
            post: match &self.stage.post {
                Some(dto) => dto.to_settings()?,
                None => buzz_scene::PostSettings::default(),
            },
            sort_by_depth: self.stage.sort_by_depth,
        };

        for (index, dto) in self.layers.iter().enumerate() {
            scene
                .edit_stage_layers()
                .insert(index, dto.to_layer(images)?);
        }

        // The library loads before anything can reference it, so an instance
        // never resolves against a half-built one.
        for folder in &self.library.folders {
            scene.library_mut().add_folder(folder);
        }
        for dto in &self.library.symbols {
            let symbol = dto.to_symbol(images)?;
            scene.library_mut().insert(symbol);
        }

        if let Some(camera) = &self.camera {
            let mut track = buzz_scene::CameraTrack::from_parts(
                camera
                    .keys
                    .iter()
                    // Clamped on the way in: a corrupt or hand-edited tilt must
                    // not produce a camera looking through the back of the scene.
                    .map(|k| camera_key_from_dto(k).clamped())
                    .collect(),
                camera.enabled,
                camera.focal_distance,
            );
            track.aperture = camera.aperture.max(0.0);
            track.focus_depth = camera.focus_depth;
            track.angles = camera
                .angles
                .iter()
                .map(|a| buzz_scene::NamedAngle {
                    name: a.name.clone(),
                    state: camera_key_from_dto(&a.state).clamped(),
                })
                .collect();
            *scene.camera_mut() = track;
        }

        // Sounds come back without their audio: the bytes live in the
        // container's `media/` directory and are reattached by the loader,
        // which is the only place that has the archive open.
        for sound in &self.sounds {
            scene.sounds_mut().insert(buzz_scene::SoundAsset {
                id: buzz_scene::SoundId(sound.id),
                name: sound.name.clone(),
                data: Arc::new(Vec::new()),
                format: sound.format.clone(),
                sample_rate: sound.sample_rate,
                channels: sound.channels,
                length: sound.length,
            });
        }

        // The palette. A file written before version 13 has none, and gets
        // the default one — the ten colours the Color panel always offered,
        // which is what such a document effectively had.
        if self.swatches.is_empty() && self.swatch_folders.is_empty() {
            *scene.swatches_mut() = buzz_scene::default_swatches();
        } else {
            let palette = scene.swatches_mut();
            for folder in &self.swatch_folders {
                palette.add_folder(folder);
            }
            for dto in &self.swatches {
                let mut swatch = buzz_scene::Swatch::new(
                    buzz_scene::SwatchId(dto.id),
                    dto.name.clone(),
                    color_from_hex(&dto.color)?,
                );
                swatch.folder = dto.folder.clone();
                palette.insert(swatch);
            }
        }

        if let Some(region) = &self.looping {
            // Clamped on the way in: a file hand-edited to loop frames that no
            // longer exist must not make the exporter read past the end.
            let frames = scene.frame_count();
            *scene.looping_mut() = buzz_scene::LoopRegion {
                enabled: true,
                start: region.start,
                end: region.end,
                repeats: region.repeats,
            }
            .clamped(frames);
        }

        if let Some(rig) = &self.lights {
            let lights = scene.lights_mut();
            lights.enabled = rig.enabled;
            lights.base = color_from_hex(&rig.base)?;
            lights.modelling = rig.modelling;

            for dto in &rig.lights {
                // An unknown kind loads as a sky rather than being dropped: a
                // light that vanishes takes the shot's look with it, and a
                // fill light is the one kind that cannot look wrong.
                let kind = match dto.kind.as_str() {
                    "gloom" => buzz_scene::LightKind::Gloom {
                        edge: dto
                            .position
                            .map(|[x, y]| buzz_geom::Point::new(x, y))
                            .unwrap_or_default(),
                        facing: dto.azimuth.unwrap_or(0.0),
                        throw: dto.throw.unwrap_or(900.0),
                        width: dto.width.unwrap_or(2400.0),
                    },
                    "sun" => buzz_scene::LightKind::Sun {
                        azimuth: dto.azimuth.unwrap_or(0.0),
                        elevation: dto.elevation.unwrap_or(0.8),
                    },
                    "lamp" => buzz_scene::LightKind::Lamp {
                        position: dto
                            .position
                            .map(|[x, y]| buzz_geom::Point::new(x, y))
                            .unwrap_or_default(),
                        height: dto.height.unwrap_or(160.0),
                        radius: dto.radius.unwrap_or(320.0),
                    },
                    _ => buzz_scene::LightKind::Sky {
                        horizon: dto
                            .horizon
                            .as_deref()
                            .map(color_from_hex)
                            .transpose()?
                            .unwrap_or(Color::from_rgb8(0x9A, 0x8C, 0x78)),
                    },
                };

                lights.lights.push(buzz_scene::Light {
                    id: buzz_scene::LightId(dto.id),
                    name: dto.name.clone(),
                    kind,
                    color: color_from_hex(&dto.color)?,
                    intensity: dto.intensity,
                    enabled: dto.enabled,
                    shadows: dto.shadows,
                    shadow_strength: dto.shadow_strength,
                    standing_height: dto.standing_height,
                    softness: dto.softness,
                    glow: dto.glow,
                    flicker: dto.flicker,
                    rim: dto.rim,
                    track: dto.track.as_ref().map(LightTrackDto::to_track).transpose()?,
                });
            }
        }

        // Raise the allocator past everything the file already uses, so a new
        // object cannot collide with a loaded one.
        scene.reserve_ids_above(self.max_id);
        Ok(scene)
    }
}

impl ObjectDto {
    fn from_object(object: &Object, max_id: &mut u64) -> Self {
        *max_id = (*max_id).max(object.id.0);
        let kind = match &object.kind {
            ObjectKind::Shape(s) => ObjectKindDto::Shape {
                path: s.path.to_svg(),
                fill: s.fill.as_ref().map(fill_to_dto),
                stroke: s.stroke.as_ref().map(stroke_to_dto),
                blend: s.blend,
            },
            ObjectKind::Group(children) => ObjectKindDto::Group {
                children: children
                    .iter()
                    .map(|c| Self::from_object(c, max_id))
                    .collect(),
            },
            ObjectKind::Instance(i) => ObjectKindDto::Instance {
                symbol: i.symbol.0,
                first_frame: i.first_frame,
                loop_mode: i.loop_mode,
                // The identity is the common case; leaving it out keeps a
                // document full of plain instances readable.
                color: (!i.color.is_identity()).then_some(ColorTransformDto {
                    multiply: i.color.multiply,
                    add: i.color.add,
                }),
            },
            ObjectKind::Armature(rig) => ObjectKindDto::Armature {
                root: [rig.armature.root.x, rig.armature.root.y],
                bones: rig
                    .armature
                    .bones
                    .iter()
                    .map(|b| BoneDto {
                        name: b.name.clone(),
                        parent: b.parent,
                        length: b.length,
                        rest_angle: b.rest_angle,
                        angle: b.angle,
                        limits: b.limits.map(|l| [l.min, l.max]),
                        pinned: b.pinned,
                    })
                    .collect(),
                parts: rig
                    .parts
                    .iter()
                    .map(|part| RigPartDto {
                        artwork: Self::from_object(&part.artwork, max_id),
                        weights: match &part.binding {
                            buzz_scene::RigBinding::Skin(skin) => skin.points.clone(),
                            buzz_scene::RigBinding::Rigid(_) => Vec::new(),
                        },
                        rigid_bone: match &part.binding {
                            buzz_scene::RigBinding::Rigid(bone) => Some(*bone),
                            buzz_scene::RigBinding::Skin(_) => None,
                        },
                    })
                    .collect(),
                poses: rig
                    .poses
                    .iter()
                    .map(|p| NamedPoseDto {
                        name: p.name.clone(),
                        angles: p.angles.clone(),
                    })
                    .collect(),
                pattern: rig.pattern.clone(),
            },
            ObjectKind::Warp(warp) => ObjectKindDto::Warp {
                path: warp.shape.path.to_svg(),
                fill: warp.shape.fill.as_ref().map(fill_to_dto),
                stroke: warp.shape.stroke.as_ref().map(stroke_to_dto),
                blend: warp.shape.blend,
                handles: warp
                    .handles
                    .iter()
                    .map(|h| HandleDto {
                        rest: [h.rest.x, h.rest.y],
                        current: [h.current.x, h.current.y],
                    })
                    .collect(),
                rigidity: warp.rigidity,
            },
        };

        Self {
            id: object.id.0,
            name: object.name.clone(),
            transform: object.transform.as_coeffs(),
            visible: object.visible,
            locked: object.locked,
            filters: object.filters.iter().map(FilterDto::from_filter).collect(),
            blend: if is_normal_object_blend(&object.blend) {
                String::new()
            } else {
                blend_name(object.blend).to_string()
            },
            // Omitted entirely when the object is flat, so a document that
            // does not use 3D is not a byte larger than it was.
            spatial: (!object.spatial.is_flat()).then_some(SpatialDto {
                rotation_x: object.spatial.rotation_x,
                rotation_y: object.spatial.rotation_y,
                rotation_z: object.spatial.rotation_z,
                z: object.spatial.z,
            }),
            // Likewise omitted when it is the centre, which is every object
            // nobody has moved a transformation point on.
            pivot: object.pivot.map(|p| [p.x, p.y]),
            modifiers: object
                .modifiers
                .iter()
                .map(ModifierDto::from_modifier)
                .collect(),
            text: object.text.as_ref().map(|t| TextDataDto {
                content: t.content.clone(),
                size: t.size,
                font: t.font.clone(),
            }),
            reverse: object
                .reverse
                .as_ref()
                .map(|r| Box::new(Self::from_object(r, max_id))),
            kind,
        }
    }

    fn to_object(&self, images: &buzz_scene::ImageLibrary) -> Result<Object, SerialError> {
        let kind = match &self.kind {
            ObjectKindDto::Shape {
                path,
                fill,
                stroke,
                blend,
            } => {
                let parsed =
                    BezPath::from_svg(path).map_err(|e| SerialError::BadPath(e.to_string()))?;
                ObjectKind::Shape(ShapeData {
                    path: parsed,
                    fill: fill
                        .as_ref()
                        .map(|f| fill_from_dto(f, images))
                        .transpose()?,
                    stroke: stroke
                        .as_ref()
                        .map(|s| stroke_from_dto(s, images))
                        .transpose()?,
                    blend: *blend,
                })
            }
            ObjectKindDto::Group { children } => ObjectKind::Group(
                children
                    .iter()
                    .map(|c| c.to_object(images).map(Arc::new))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ObjectKindDto::Instance {
                symbol,
                first_frame,
                loop_mode,
                color,
            } => ObjectKind::Instance(SymbolInstance {
                symbol: SymbolId(*symbol),
                first_frame: *first_frame,
                loop_mode: *loop_mode,
                color: color.map_or_else(ColorTransform::default, |c| ColorTransform {
                    multiply: c.multiply,
                    add: c.add,
                }),
            }),
            ObjectKindDto::Armature {
                root,
                bones,
                parts,
                poses,
                pattern,
            } => {
                let mut armature = buzz_rig::Armature::new(buzz_geom::Point::new(root[0], root[1]));
                for dto in bones {
                    // Through `push`, which refuses a parent that comes later:
                    // a hand-edited or corrupted file must not be able to
                    // describe a cycle the solver would walk forever.
                    armature.push(buzz_rig::Bone {
                        name: dto.name.clone(),
                        parent: dto.parent,
                        length: dto.length,
                        rest_angle: dto.rest_angle,
                        angle: dto.angle,
                        limits: dto
                            .limits
                            .map(|[min, max]| buzz_rig::JointLimits::new(min, max)),
                        pinned: dto.pinned,
                    });
                }

                let mut rig = buzz_scene::ArmatureData::new(armature);
                for part in parts {
                    let artwork = Arc::new(part.artwork.to_object(images)?);
                    let binding = match part.rigid_bone {
                        Some(bone) => buzz_scene::RigBinding::Rigid(bone),
                        None => buzz_scene::RigBinding::Skin(buzz_rig::SkinBinding {
                            points: part.weights.clone(),
                        }),
                    };
                    rig.parts.push(buzz_scene::RigPart { artwork, binding });
                }
                rig.poses = poses
                    .iter()
                    .map(|p| buzz_scene::NamedPose {
                        name: p.name.clone(),
                        angles: p.angles.clone(),
                    })
                    .collect();
                rig.pattern = pattern.clone();
                ObjectKind::Armature(rig)
            }
            ObjectKindDto::Warp {
                path,
                fill,
                stroke,
                blend,
                handles,
                rigidity,
            } => {
                let parsed =
                    BezPath::from_svg(path).map_err(|e| SerialError::BadPath(e.to_string()))?;
                let shape = ShapeData {
                    path: parsed,
                    fill: fill
                        .as_ref()
                        .map(|f| fill_from_dto(f, images))
                        .transpose()?,
                    stroke: stroke
                        .as_ref()
                        .map(|s| stroke_from_dto(s, images))
                        .transpose()?,
                    blend: *blend,
                };
                let mut warp = buzz_scene::WarpData::new(shape);
                warp.rigidity = *rigidity;
                warp.handles = handles
                    .iter()
                    .map(|h| buzz_rig::WarpHandle {
                        rest: buzz_geom::Point::new(h.rest[0], h.rest[1]),
                        current: buzz_geom::Point::new(h.current[0], h.current[1]),
                    })
                    .collect();
                ObjectKind::Warp(warp)
            }
        };

        let mut filters = Vec::with_capacity(self.filters.len());
        for filter in &self.filters {
            filters.push(filter.to_filter()?);
        }

        Ok(Object {
            id: ObjectId(self.id),
            name: self.name.clone(),
            transform: Affine::new(self.transform),
            kind,
            locked: self.locked,
            visible: self.visible,
            filters,
            blend: blend_from(&self.blend),
            spatial: self
                .spatial
                .map(|s| buzz_scene::Spatial {
                    rotation_x: sane_angle(s.rotation_x),
                    rotation_y: sane_angle(s.rotation_y),
                    rotation_z: sane_angle(s.rotation_z),
                    z: if s.z.is_finite() { s.z } else { 0.0 },
                })
                .unwrap_or_default(),
            // A point that is not a number would put the artwork nowhere the
            // moment it was rotated; treat it as "not set" and use the centre.
            pivot: self
                .pivot
                .filter(|p| p[0].is_finite() && p[1].is_finite())
                .map(|p| buzz_geom::Point::new(p[0], p[1])),
            modifiers: self.modifiers.iter().filter_map(ModifierDto::to_modifier).collect(),
            text: self.text.as_ref().map(|t| buzz_scene::TextData {
                content: t.content.clone(),
                size: t.size,
                font: t.font.clone(),
            }),
            reverse: self
                .reverse
                .as_ref()
                .map(|r| r.to_object(images).map(std::sync::Arc::new))
                .transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Point, Shape as _};
    use buzz_scene::Tween;
    use kurbo::{Circle, Rect};

    fn sample_scene() -> Scene {
        let mut scene = Scene::empty();
        let base = scene.add_layer("Background", LayerKind::Normal);
        let art = scene.add_layer("Artwork", LayerKind::Normal);

        scene.add_shape(
            base,
            ShapeData::filled(
                Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9),
                Color::from_rgb8(0x22, 0x44, 0x88),
            ),
        );
        scene.add_shape(
            art,
            ShapeData::stroked(
                // A realistic authoring tolerance. kurbo grows a circle's
                // segment count from `radius / tolerance`, so a stress value
                // like 1e-9 would yield ~60 cubics where a drawing tool
                // produces 4.
                Circle::new(Point::new(100.0, 100.0), 40.0).to_path(0.05),
                Color::from_rgb8(0xFF, 0x00, 0x66),
                2.5,
            ),
        );
        scene.update_layer(art, |l| {
            l.locked = true;
            l.outline = true;
            l.alpha = 0.4;
            l.height = LayerHeight::Double;
        });
        scene
    }

    #[test]
    fn colours_round_trip_through_hex() {
        for c in [
            Color::WHITE,
            Color::BLACK,
            Color::from_rgb8(0x12, 0x34, 0x56),
            Color::from_rgba8(0xAB, 0xCD, 0xEF, 0x80),
        ] {
            let hex = color_to_hex(c);
            let back = color_from_hex(&hex).unwrap();
            assert_eq!(
                c.to_rgba8().to_u8_array(),
                back.to_rgba8().to_u8_array(),
                "colour changed through {hex}"
            );
        }
    }

    #[test]
    fn opaque_colours_are_written_without_an_alpha_byte() {
        assert_eq!(color_to_hex(Color::from_rgb8(0x12, 0x34, 0x56)), "#123456");
        assert_eq!(
            color_to_hex(Color::from_rgba8(0x12, 0x34, 0x56, 0x7F)),
            "#1234567F"
        );
    }

    #[test]
    fn malformed_colours_are_rejected_clearly() {
        for bad in ["", "#12", "#12345", "nonsense", "#GGGGGG"] {
            assert!(
                color_from_hex(bad).is_err(),
                "{bad:?} should not have parsed"
            );
        }
        // Both with and without the hash are accepted.
        assert!(color_from_hex("123456").is_ok());
    }

    /// Paths must survive the round trip *exactly*, including the extreme
    /// coordinates the unbounded-zoom design produces.
    #[test]
    fn path_round_trip_is_lossless_even_at_extreme_scales() {
        let mut path = BezPath::new();
        path.move_to(Point::new(1234.5678901234567, -987.6543210987654));
        path.line_to(Point::new(1e-9, 1e9));
        path.curve_to(
            Point::new(0.1, 0.2),
            Point::new(1e-12, 3.0),
            Point::new(1e6 + 1e-7, 2.5),
        );
        path.quad_to(Point::new(5.5, 6.5), Point::new(7.25, 8.125));
        path.close_path();

        let svg = path.to_svg();
        let back = BezPath::from_svg(&svg).expect("our own output must parse");

        assert_eq!(
            path.elements(),
            back.elements(),
            "path changed through SVG round trip:\n{svg}"
        );
    }

    #[test]
    fn a_scene_survives_a_full_round_trip() {
        let scene = sample_scene();
        let dto = DocumentDto::from_scene(&scene);
        let back = dto.to_scene().unwrap();

        assert_eq!(back.stage().size, scene.stage().size);
        assert_eq!(back.stage().frame_rate, scene.stage().frame_rate);
        assert_eq!(back.layers().len(), scene.layers().len());
        assert_eq!(back.shape_count(), scene.shape_count());

        for (a, b) in scene.layers().iter().zip(back.layers().iter()) {
            assert_eq!(a.id, b.id, "layer ids must be preserved");
            assert_eq!(a.name, b.name);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.locked, b.locked);
            assert_eq!(a.outline, b.outline);
            assert_eq!(a.alpha, b.alpha);
            assert_eq!(a.height, b.height);
            assert_eq!(a.objects_at(0).len(), b.objects_at(0).len());
        }
    }

    #[test]
    fn layer_order_is_preserved() {
        let mut scene = Scene::empty();
        for i in 0..5 {
            scene.add_layer(format!("Layer {i}"), LayerKind::Normal);
        }
        let before: Vec<String> = scene.layers().iter().map(|l| l.name.clone()).collect();

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let after: Vec<String> = back.layers().iter().map(|l| l.name.clone()).collect();

        assert_eq!(before, after, "front-to-back order must survive");
    }

    #[test]
    fn nested_groups_and_transforms_survive() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("L", LayerKind::Normal);

        let leaf = Arc::new(
            Object::shape(
                ObjectId(90),
                ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
            )
            .with_transform(Affine::translate((3.0, 4.0))),
        );
        let inner = Arc::new(Object::group(ObjectId(91), vec![leaf]));
        let outer = Object::group(ObjectId(92), vec![inner])
            .with_transform(Affine::scale(2.0))
            .with_name("nest");
        scene.add_object(layer, outer).unwrap();

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let (_, restored) = back.find_object(ObjectId(92)).unwrap();

        assert_eq!(restored.name.as_deref(), Some("nest"));
        assert_eq!(restored.shape_count(), 1);
        assert_eq!(
            restored.transform.as_coeffs(),
            Affine::scale(2.0).as_coeffs()
        );
        assert_eq!(
            restored.bounds(),
            scene.find_object(ObjectId(92)).unwrap().1.bounds()
        );
    }

    /// Layer parenting is a link between layers, and a link is exactly the
    /// sort of thing a format loses quietly: the file still opens, the artwork
    /// is all there, and the rig no longer moves together.
    #[test]
    fn layer_parenting_survives_a_round_trip() {
        let mut scene = Scene::empty();
        let body = scene.add_layer("Body", LayerKind::Normal);
        let head = scene.add_layer("Head", LayerKind::Normal);
        let hat = scene.add_layer("Hat", LayerKind::Normal);
        scene.update_layer(head, |l| l.follows = Some(body));
        scene.update_layer(hat, |l| l.follows = Some(head));

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        assert_eq!(back.layers().get(head).unwrap().follows, Some(body));
        assert_eq!(back.layers().get(hat).unwrap().follows, Some(head));
        assert_eq!(
            back.layers().get(body).unwrap().follows,
            None,
            "the layer at the top of the chain follows nothing"
        );
    }

    /// The transformation point is part of the artwork's description: a hinge
    /// that moved back to the middle when the file was reopened would silently
    /// change every rotation after it.
    #[test]
    fn a_transformation_point_survives_a_round_trip() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 80.0, 20.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a shape");
        scene.set_pivot_at(0, id, buzz_geom::Point::new(0.0, 10.0));

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let (_, object) = back.find_object(id).expect("the object");
        assert_eq!(object.pivot, Some(buzz_geom::Point::new(0.0, 10.0)));
    }

    /// Live modifiers — a wiggle and a spring, with all their parameters — come
    /// back exactly as they went in.
    #[test]
    fn modifiers_survive_a_round_trip() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a shape");
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(buzz_scene::Modifier::Wiggle {
                amplitude: 7.5,
                frequency: 2.0,
            });
            o.modifiers.push(buzz_scene::Modifier::Spring {
                root: 3,
                stiffness: 90.0,
                damping: 8.0,
                coupling: 0.02,
            });
            o.modifiers.push(buzz_scene::Modifier::LookAt { x: 12.5, y: -30.0 });
            o.modifiers
                .push(buzz_scene::Modifier::AutoSquashStretch { amount: 0.015 });
        });

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let (_, object) = back.find_object(id).expect("the object");
        assert_eq!(
            object.modifiers,
            vec![
                buzz_scene::Modifier::Wiggle {
                    amplitude: 7.5,
                    frequency: 2.0
                },
                buzz_scene::Modifier::Spring {
                    root: 3,
                    stiffness: 90.0,
                    damping: 8.0,
                    coupling: 0.02
                },
                buzz_scene::Modifier::LookAt { x: 12.5, y: -30.0 },
                buzz_scene::Modifier::AutoSquashStretch { amount: 0.015 },
            ]
        );
    }

    /// A text object's string and size survive a round trip (the outlines are an
    /// ordinary shape path and serialise separately).
    #[test]
    fn text_metadata_survives_a_round_trip() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 40.0, 20.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a shape");
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.text = Some(buzz_scene::TextData {
                content: "Hello".to_string(),
                size: 36.0,
                font: Some("Nirmala UI".to_string()),
            });
        });

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let (_, object) = back.find_object(id).expect("the object");
        assert_eq!(
            object.text,
            Some(buzz_scene::TextData {
                content: "Hello".to_string(),
                size: 36.0,
                font: Some("Nirmala UI".to_string()),
            })
        );

        // An object without text writes no `text` key.
        let plain = serde_json::to_string(&DocumentDto::from_scene(&Scene::empty())).unwrap();
        assert!(!plain.contains("\"text\""), "no text object should write the key");
    }

    /// A tiling image fill keeps its `tile` flag through the paint conversion,
    /// and a non-tiling one writes no `tile` key (so old files stay byte-identical
    /// and load as `tile: false`).
    #[test]
    fn image_fill_tile_survives_and_stays_optional() {
        use std::sync::Arc;
        // A library holding one procedural-style asset the fill can resolve to.
        let mut images = buzz_scene::ImageLibrary::default();
        let asset = buzz_scene::ImageAsset::from_pixels(
            buzz_scene::ImageId(7),
            "tex",
            2,
            2,
            Arc::new(vec![0xFFu8; 2 * 2 * 4]),
        );
        let asset = images.insert(asset);

        let tiled = Paint::Image(Box::new(buzz_scene::ImageFill::tiled(asset, 32.0)));
        assert!(matches!(&tiled, Paint::Image(i) if i.tile), "tiled() sets the flag");

        // to_dto then from_dto (against the library) preserves tile.
        let (_c, _g, dto) = paint_to_dto(&tiled);
        let dto = dto.expect("an image dto");
        assert!(dto.tile, "the dto carries tile");
        let back = paint_from_dto(None, None, Some(&dto), &images).unwrap();
        assert!(matches!(back, Paint::Image(i) if i.tile), "tile came back");

        // A tiling fill writes the key; a non-tiling one omits it.
        assert!(serde_json::to_string(&dto).unwrap().contains("\"tile\""));
        let plain = ImageFillDto { image: 7, transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], smooth: true, tile: false };
        assert!(
            !serde_json::to_string(&plain).unwrap().contains("\"tile\""),
            "a non-tiling fill must not write the key"
        );
        // And a JSON without the key loads as tile: false.
        let loaded: ImageFillDto =
            serde_json::from_str(r#"{"image":7,"transform":[1,0,0,1,0,0]}"#).unwrap();
        assert!(!loaded.tile, "absent tile loads false");
    }

    /// A reverse (back) drawing — a whole nested object — round-trips, and an
    /// object without one writes no `reverse` key.
    #[test]
    fn a_reverse_drawing_survives_a_round_trip() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let front = scene
            .add_shape(
                layer,
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a front");
        scene.update_object(front, |o| {
            o.reverse = Some(std::sync::Arc::new(buzz_scene::Object::shape(
                buzz_scene::ObjectId(4242),
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9),
                    Color::BLACK,
                ),
            )));
        });

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let (_, object) = back.find_object(front).expect("the front object");
        assert!(object.reverse.is_some(), "the back view came back");

        let plain = serde_json::to_string(&DocumentDto::from_scene(&Scene::empty())).unwrap();
        assert!(!plain.contains("\"reverse\""), "no back → no key");
    }

    /// An object with no modifiers writes no `modifiers` key, so a document that
    /// uses none is byte-identical to what version 26 wrote.
    #[test]
    fn an_object_without_modifiers_writes_no_field() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene
            .add_shape(
                layer,
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a shape");
        let json = serde_json::to_string(&DocumentDto::from_scene(&scene)).unwrap();
        assert!(!json.contains("modifiers"), "the empty stack should not be written");
    }

    /// A file written before version 27 has no `modifiers` key; it must load with
    /// an empty stack rather than fail.
    #[test]
    fn a_file_from_before_modifiers_still_loads() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a shape");
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(buzz_scene::Modifier::Wiggle {
                amplitude: 5.0,
                frequency: 1.0,
            });
        });

        let mut json = serde_json::to_value(&DocumentDto::from_scene(&scene)).unwrap();
        fn strip(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(map) => {
                    map.remove("modifiers");
                    for (_, v) in map.iter_mut() {
                        strip(v);
                    }
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
                _ => {}
            }
        }
        strip(&mut json);

        let back: DocumentDto = serde_json::from_value(json).expect("deserialise");
        let loaded = back.to_scene().expect("to scene");
        let (_, object) = loaded.find_object(id).expect("the object");
        assert!(object.modifiers.is_empty(), "an old file must load with no modifiers");
    }

    /// Every part of a gradient comes back: the stops with their offsets, the
    /// placement matrix, the spread and the focal point.
    #[test]
    fn a_gradient_survives_a_round_trip() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);

        let mut gradient = buzz_scene::Gradient::new(
            buzz_scene::GradientKind::Radial,
            vec![
                buzz_scene::GradientStop::new(0.0, Color::from_rgb8(0xFF, 0x00, 0x00)),
                buzz_scene::GradientStop::new(0.3, Color::from_rgba8(0x00, 0xFF, 0x00, 0x80)),
                buzz_scene::GradientStop::new(1.0, Color::from_rgb8(0x00, 0x00, 0xFF)),
            ],
        );
        gradient.transform = Affine::new([2.0, 0.5, -0.25, 3.0, 40.0, 60.0]);
        gradient.spread = buzz_scene::GradientSpread::Reflect;
        gradient.focal = -0.4;

        let id = scene
            .add_shape(
                layer,
                ShapeData {
                    path: kurbo::Rect::new(0.0, 0.0, 80.0, 20.0).to_path(1e-9),
                    fill: Some(buzz_scene::FillSpec::gradient(gradient.clone())),
                    stroke: None,
                    blend: PaintBlend::Normal,
                },
            )
            .expect("a shape");

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let (_, object) = back.find_object(id).expect("the object");
        let ObjectKind::Shape(shape) = &object.kind else {
            panic!("expected a shape")
        };
        let read = shape
            .fill
            .as_ref()
            .expect("filled")
            .paint
            .gradient()
            .expect("it should still be a gradient");

        assert_eq!(read, &gradient);
    }

    /// A gradient fill writes **no** `color`, and a solid fill writes no
    /// `gradient`. Writing both would put two answers in the file for one
    /// question, and the day they disagreed the document would be wrong in a
    /// way nothing could adjudicate.
    #[test]
    fn a_fill_writes_a_colour_or_a_gradient_and_never_both() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let square = kurbo::Rect::new(0.0, 0.0, 10.0, 10.0);
        scene
            .add_shape(layer, ShapeData::filled(square.to_path(1e-9), Color::WHITE))
            .expect("a shape");

        let solid = serde_json::to_string(&DocumentDto::from_scene(&scene)).unwrap();
        assert!(
            !solid.contains("gradient"),
            "a document with no gradient should not mention one: {solid}"
        );

        let mut with_ramp = Scene::empty();
        let layer = with_ramp.add_layer("Art", LayerKind::Normal);
        with_ramp
            .add_shape(
                layer,
                ShapeData {
                    path: square.to_path(1e-9),
                    fill: Some(buzz_scene::FillSpec::gradient(
                        buzz_scene::Gradient::linear(Color::BLACK, Color::WHITE, square),
                    )),
                    stroke: None,
                    blend: PaintBlend::Normal,
                },
            )
            .expect("a shape");

        let dto = DocumentDto::from_scene(&with_ramp);
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("gradient"), "{json}");

        // The fill object itself carries the gradient and no colour.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let fill = find_fill(&value).expect("a fill in the document");
        assert!(fill.get("gradient").is_some(), "{fill:?}");
        assert!(
            fill.get("color").is_none(),
            "a gradient fill must not also carry a flat colour: {fill:?}"
        );
    }

    /// Walk the serialised document for the first `fill` object.
    fn find_fill(v: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
        match v {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::Object(fill)) = map.get("fill") {
                    return Some(fill);
                }
                map.values().find_map(find_fill)
            }
            serde_json::Value::Array(items) => items.iter().find_map(find_fill),
            _ => None,
        }
    }

    /// **Every file written before version 16 still loads.** They carry `color`
    /// and no `gradient`, which is exactly what a solid paint reads back as.
    #[test]
    fn a_pre_gradient_file_still_loads_its_fills() {
        let older = serde_json::json!({
            "color": "#FF8800",
            "rule": "NonZero",
        });
        let dto: FillDto = serde_json::from_value(older).expect("version 15's shape still parses");
        let fill =
            fill_from_dto(&dto, &buzz_scene::ImageLibrary::default()).expect("it should read");
        assert_eq!(
            fill.color().to_rgba8().to_u8_array(),
            [0xFF, 0x88, 0x00, 0xFF]
        );
        assert!(!fill.paint.is_gradient());
    }

    /// A fill with neither field is a damaged file, not a reason to refuse the
    /// whole document — losing every other shape over one missing colour would
    /// be the worse outcome.
    #[test]
    fn a_fill_with_no_paint_at_all_reads_as_black() {
        let dto: FillDto = serde_json::from_value(serde_json::json!({})).expect("parses");
        let fill =
            fill_from_dto(&dto, &buzz_scene::ImageLibrary::default()).expect("it should read");
        assert_eq!(fill.color().to_rgba8().to_u8_array(), [0, 0, 0, 255]);
    }

    /// An object nobody has touched writes no point at all, so a document that
    /// does not use the feature is byte-for-byte what it always was.
    #[test]
    fn an_untouched_object_writes_no_transformation_point() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene
            .add_shape(
                layer,
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 80.0, 20.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a shape");

        let json = serde_json::to_string(&DocumentDto::from_scene(&scene)).unwrap();
        assert!(!json.contains("pivot"), "{json}");

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        assert!(
            back.layers()
                .iter()
                .all(|l| l.objects_at(0).iter().all(|o| o.pivot.is_none()))
        );
    }

    /// The palette is part of the document: names, colours and the folders
    /// they are filed in all have to come back.
    #[test]
    fn the_palette_survives_a_round_trip() {
        let mut scene = Scene::default();
        scene.swatches_mut().add_folder("Hero/Skin");
        let id = scene.add_swatch(
            "Skin Shadow",
            Color::from_rgb8(0xC0, 0x8A, 0x6E),
            Some("Hero/Skin".into()),
        );
        let before = scene.swatches().len();

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();

        assert_eq!(back.swatches().len(), before);
        let swatch = back.swatches().get(id).expect("the named colour");
        assert_eq!(swatch.name, "Skin Shadow");
        assert_eq!(swatch.folder.as_deref(), Some("Hero/Skin"));
        assert_eq!(swatch.color.to_rgba8().to_u8_array()[0], 0xC0);
    }

    /// An empty folder is a decision too — made in advance of the colours that
    /// will go in it — so it must not be dropped by a save.
    #[test]
    fn an_empty_palette_folder_survives() {
        let mut scene = Scene::default();
        scene.swatches_mut().add_folder("Backgrounds");

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        assert!(back.swatches().folders().any(|f| f == "Backgrounds"));
    }

    /// A file written before version 13 has no palette at all, and gets the
    /// default one — which is what such a document effectively had, since the
    /// Color panel always offered those ten colours.
    #[test]
    fn a_document_without_a_palette_gets_the_default_one() {
        let scene = Scene::default();
        let mut json = serde_json::to_value(DocumentDto::from_scene(&scene)).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("swatches");
        object.remove("swatch_folders");

        let dto: DocumentDto = serde_json::from_value(json).unwrap();
        let back = dto.to_scene().unwrap();

        assert_eq!(back.swatches().len(), buzz_scene::default_palette().len());
        assert!(back.swatches().find_color(Color::BLACK).is_some());
    }

    /// The looping section is part of the document, so it has to survive being
    /// saved — a loop that only lasts as long as the session would mean the
    /// exported film changes depending on whether the file was reopened.
    #[test]
    fn a_looping_section_survives_a_round_trip() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(9);
        });
        *scene.looping_mut() = buzz_scene::LoopRegion {
            enabled: true,
            start: 2,
            end: 5,
            repeats: 4,
        };

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        assert_eq!(*back.looping(), *scene.looping());
        assert_eq!(back.rendered_frame_count(), scene.rendered_frame_count());
    }

    /// Most documents do not loop, and those must be byte-for-byte what they
    /// always were: no key in the JSON, and nothing looping on the way back.
    #[test]
    fn a_document_that_does_not_loop_writes_no_loop() {
        let mut scene = Scene::empty();
        scene.add_layer("Art", LayerKind::Normal);

        let json = serde_json::to_value(DocumentDto::from_scene(&scene)).unwrap();
        assert!(
            json.get("looping").is_none(),
            "a document with no loop should not mention one: {json}"
        );

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        assert!(!back.looping().enabled);
        assert_eq!(back.rendered_frame_count(), back.frame_count());
    }

    /// A file hand-edited (or written by a future build) to loop frames this
    /// document does not have must not make the exporter read past the end.
    #[test]
    fn a_loop_past_the_end_of_the_document_is_brought_back() {
        let mut scene = Scene::empty();
        scene.add_layer("Art", LayerKind::Normal);
        *scene.looping_mut() = buzz_scene::LoopRegion {
            enabled: true,
            start: 400,
            end: 900,
            repeats: 3,
        };

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let last = back.frame_count().saturating_sub(1);
        assert!(back.looping().end <= last, "{:?}", back.looping());
        assert!(back.playlist().iter().all(|f| *f < back.frame_count()));
    }

    /// A document written before layer parenting existed has no such field,
    /// and must load with every layer following nothing rather than refusing.
    #[test]
    fn a_document_without_the_field_loads_with_no_parenting() {
        let mut scene = Scene::empty();
        scene.add_layer("Body", LayerKind::Normal);

        let mut json = serde_json::to_value(DocumentDto::from_scene(&scene)).unwrap();
        for layer in json["layers"].as_array_mut().unwrap() {
            layer.as_object_mut().unwrap().remove("follows");
        }
        let dto: DocumentDto = serde_json::from_value(json).unwrap();
        let back = dto.to_scene().unwrap();

        assert!(back.layers().iter().all(|l| l.follows.is_none()));
    }

    /// Filters are the sort of thing a format loses quietly: the artwork is
    /// all there and the shot has gone flat.
    #[test]
    fn filters_and_blend_modes_survive_a_round_trip() {
        use buzz_scene::{Blend, ColorAdjust, Filter, FilterKind, Quality};

        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(
                    Rect::new(0.0, 0.0, 40.0, 40.0).to_path(1e-9),
                    Color::from_rgb8(0x40, 0x80, 0xC0),
                ),
            )
            .expect("the shape");

        scene.update_object(id, |o| {
            o.blend = Blend::Multiply;
            o.filters = vec![
                Filter::new(FilterKind::DropShadow {
                    x: 12.0,
                    y: 3.0,
                    strength: 0.6,
                    angle: 1.25,
                    distance: 9.0,
                    color: Color::from_rgb8(0x10, 0x20, 0x30),
                    inner: true,
                    knockout: false,
                    hide_object: true,
                    quality: Quality::High,
                }),
                Filter {
                    kind: FilterKind::Adjust(ColorAdjust {
                        brightness: 12.0,
                        contrast: -4.0,
                        saturation: 30.0,
                        hue: 90.0,
                    }),
                    enabled: false,
                },
            ];
        });
        // A layer filter too — this program has them and Animate does not.
        scene.update_layer(layer, |l| {
            l.filters = vec![Filter::new(FilterKind::blur())];
        });

        let back = DocumentDto::from_scene(&scene).to_scene().unwrap();
        let (_, object) = back.find_object(id).expect("the shape");

        assert_eq!(object.blend, Blend::Multiply);
        assert_eq!(object.filters.len(), 2);
        assert_eq!(
            object.filters[0].kind,
            scene.find_object(id).unwrap().1.filters[0].kind
        );
        assert!(!object.filters[1].enabled, "a disabled filter stays off");
        assert_eq!(back.layers().get(layer).unwrap().filters.len(), 1);
    }

    /// A document from before filters existed has no such field, and must load
    /// with none rather than refusing.
    #[test]
    fn a_document_without_filters_loads_with_none() {
        let scene = sample_scene();
        let mut json = serde_json::to_value(DocumentDto::from_scene(&scene)).unwrap();
        for layer in json["layers"].as_array_mut().unwrap() {
            let layer = layer.as_object_mut().unwrap();
            layer.remove("filters");
            for keyframe in layer["keyframes"].as_array_mut().unwrap() {
                for object in keyframe.as_object_mut().unwrap()["objects"]
                    .as_array_mut()
                    .unwrap()
                {
                    let object = object.as_object_mut().unwrap();
                    object.remove("filters");
                    object.remove("blend");
                }
            }
        }
        let dto: DocumentDto = serde_json::from_value(json).unwrap();
        let back = dto.to_scene().unwrap();
        assert!(back.layers().iter().all(|l| l.filters.is_empty()));
    }

    /// A filter kind this build does not know is refused rather than dropped:
    /// losing an effect silently is worse than saying the file is wrong.
    #[test]
    fn an_unknown_filter_kind_is_refused() {
        let dto = FilterDto {
            kind: "kaleidoscope".into(),
            enabled: true,
            x: None,
            y: None,
            strength: None,
            angle: None,
            distance: None,
            color: None,
            highlight: None,
            shadow: None,
            inner: false,
            knockout: false,
            hide_object: false,
            quality: None,
            bevel: None,
            brightness: None,
            contrast: None,
            saturation: None,
            hue: None,
        };
        assert!(dto.to_filter().is_err());
    }

    #[test]
    fn a_gradient_map_filter_round_trips() {
        let filter = buzz_scene::Filter::new(buzz_scene::FilterKind::GradientMap(
            buzz_scene::GradientMap {
                shadow: Color::from_rgb8(0x11, 0x22, 0x33),
                highlight: Color::from_rgb8(0xEE, 0xDD, 0xCC),
            },
        ));
        let dto = FilterDto::from_filter(&filter);
        assert_eq!(dto.kind, "gradientmap");
        assert_eq!(dto.to_filter().unwrap(), filter);
    }

    #[test]
    fn ids_are_reserved_after_loading() {
        let scene = sample_scene();
        let mut back = DocumentDto::from_scene(&scene).to_scene().unwrap();

        let existing: Vec<u64> = back
            .layers()
            .iter()
            .flat_map(|l| l.all_objects().map(|o| o.id.0))
            .chain(back.layers().iter().map(|l| l.id.0))
            .collect();
        let fresh = back.next_object_id();

        assert!(
            !existing.contains(&fresh.0),
            "new id {} collided with one already in the file",
            fresh.0
        );
    }

    /// A library with folders, an instance and a tween must come back intact —
    /// this is the whole of what format 3 added.
    #[test]
    fn symbols_instances_and_tweens_survive_a_round_trip() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Stage", LayerKind::Normal);

        // An empty folder, and one holding a symbol.
        scene.library_mut().add_folder("Characters/Hero");
        scene.library_mut().add_folder("Unused");
        let symbol = scene.add_symbol(
            "Ball",
            buzz_scene::SymbolKind::MovieClip,
            Some("Characters/Hero"),
        );

        let inner_layer = scene
            .library()
            .get(symbol)
            .unwrap()
            .layers
            .iter()
            .next()
            .unwrap()
            .id;
        scene.library_mut().update(symbol, |s| {
            s.layers.update(inner_layer, |l| {
                l.push_object_at(
                    0,
                    Arc::new(Object::shape(
                        ObjectId(500),
                        ShapeData::filled(
                            Circle::new(Point::new(0.0, 0.0), 20.0).to_path(0.05),
                            Color::from_rgb8(0xFF, 0x88, 0x00),
                        ),
                    )),
                );
            });
        });

        let instance = scene
            .add_instance_at(layer, 0, symbol, Affine::translate((100.0, 50.0)))
            .expect("symbol exists");

        // A classic tween with a non-default ease and extra rotations, plus
        // the instance overrides the panel would set.
        scene.edit_layers().update(layer, |l| {
            for object in l.frames.objects_at_mut(0).expect("keyframe 0 exists") {
                if let ObjectKind::Instance(i) = &mut Arc::make_mut(object).kind {
                    i.first_frame = 3;
                    i.loop_mode = LoopMode::PlayOnce;
                    i.color = ColorTransform::alpha(0.4);
                }
            }
            l.frames.insert_keyframe(11);
            l.frames.set_tween(
                0,
                Tween {
                    kind: buzz_scene::TweenKind::Classic,
                    easing: buzz_scene::Easing::Strength(-40.0),
                    extra_rotations: 2,
                    orient_to_path: true,
                },
            );
        });

        let dto = DocumentDto::from_scene(&scene);
        assert_eq!(dto.format_version, FORMAT_VERSION);
        let json = serde_json::to_string(&dto).unwrap();
        let back = serde_json::from_str::<DocumentDto>(&json)
            .unwrap()
            .to_scene()
            .unwrap();

        // The library, including the folder nobody put anything in.
        assert_eq!(back.library().len(), 1);
        let restored = back.library().get(symbol).expect("symbol id preserved");
        assert_eq!(restored.name, "Ball");
        assert_eq!(restored.kind, buzz_scene::SymbolKind::MovieClip);
        assert_eq!(restored.folder.as_deref(), Some("Characters/Hero"));
        assert_eq!(
            restored.objects_at(0).len(),
            1,
            "symbol artwork must survive"
        );
        let folders: Vec<&String> = back.library().folders().collect();
        assert!(
            folders.iter().any(|f| f.as_str() == "Unused"),
            "an empty folder must survive; got {folders:?}"
        );

        // The instance, with its overrides.
        let (_, object) = back.find_object(instance).expect("instance preserved");
        let i = object.instance().expect("still an instance");
        assert_eq!(i.symbol, symbol);
        assert_eq!(i.first_frame, 3);
        assert_eq!(i.loop_mode, LoopMode::PlayOnce);
        assert!(!i.color.is_identity(), "the colour effect was dropped");

        // The tween.
        let tween = back.layers().get(layer).unwrap().frames.tween_at(0);
        assert_eq!(tween.kind, buzz_scene::TweenKind::Classic);
        assert_eq!(tween.easing, buzz_scene::Easing::Strength(-40.0));
        assert_eq!(tween.extra_rotations, 2);
        assert!(tween.orient_to_path);

        // The allocator must clear the symbol's id too, not just the stage's.
        let mut back = back;
        let fresh = back.next_object_id();
        assert!(fresh.0 > symbol.0, "a new id would collide with the symbol");
    }

    /// A version 2 file has no library and no tweens. It must still load.
    #[test]
    fn a_version_two_document_loads_without_a_library() {
        let json = r##"{
            "format_version": 2,
            "stage": { "width": 550, "height": 400, "background": "#FFFFFF", "frame_rate": 24 },
            "layers": [
                { "id": 1, "name": "Layer_1", "kind": "Normal", "visible": true,
                  "locked": false, "color": "#0099FF", "length": 5,
                  "keyframes": [ { "start": 0, "objects": [] } ] }
            ],
            "max_id": 1
        }"##;
        let scene = serde_json::from_str::<DocumentDto>(json)
            .unwrap()
            .to_scene()
            .unwrap();
        assert!(scene.library().is_empty());
        assert_eq!(scene.layers().get(LayerId(1)).unwrap().frames.length(), 5);
        assert!(
            !scene
                .layers()
                .get(LayerId(1))
                .unwrap()
                .frames
                .tween_at(0)
                .is_active()
        );
    }

    #[test]
    fn a_future_format_version_is_refused_rather_than_misread() {
        let mut dto = DocumentDto::from_scene(&sample_scene());
        dto.format_version = FORMAT_VERSION + 5;
        assert!(matches!(
            dto.to_scene(),
            Err(SerialError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn corrupt_path_data_produces_an_error_not_a_panic() {
        let mut dto = DocumentDto::from_scene(&sample_scene());
        let corrupted = dto
            .layers
            .iter_mut()
            .flat_map(|l| l.keyframes.iter_mut())
            .flat_map(|k| k.objects.iter_mut())
            .find_map(|o| match &mut o.kind {
                ObjectKindDto::Shape { path, .. } => {
                    *path = "this is not a path".into();
                    Some(())
                }
                _ => None,
            });
        assert!(corrupted.is_some(), "the sample should contain a shape");
        assert!(matches!(dto.to_scene(), Err(SerialError::BadPath(_))));
    }

    #[test]
    fn json_is_stable_and_reasonably_compact() {
        let scene = sample_scene();
        let dto = DocumentDto::from_scene(&scene);

        let a = serde_json::to_string(&dto).unwrap();
        let b = serde_json::to_string(&DocumentDto::from_scene(&scene)).unwrap();
        assert_eq!(a, b, "serialising twice must produce identical bytes");

        // Round-trip through JSON, not just through the DTO.
        let parsed: DocumentDto = serde_json::from_str(&a).unwrap();
        assert_eq!(
            parsed.to_scene().unwrap().shape_count(),
            scene.shape_count()
        );

        // Cost is dominated by path data, so measure per segment rather than
        // in absolute bytes — the latter just tracks how detailed the test
        // artwork happens to be.
        //
        // Coordinates are written at full `f64` precision, which is verbose
        // (`1234.5678901234567` is 18 characters). That is a deliberate
        // trade: losslessness matters more than JSON size, and the container's
        // deflate absorbs most of it. Storing `PathEl` enums as JSON objects
        // would be several times worse *and* no more accurate.
        let segments: usize = scene
            .layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .map(|o| match &o.kind {
                buzz_scene::ObjectKind::Shape(s) => s.path.elements().len(),
                _ => 0,
            })
            .sum();
        let per_segment = a.len() / segments.max(1);
        assert!(
            per_segment < 200,
            "{} bytes for {segments} segments ({per_segment} each) is too verbose",
            a.len()
        );
    }

    #[test]
    fn optional_fields_may_be_absent_from_json() {
        // A minimal hand-written document must load, so the format is
        // hand-editable and tolerant of older files.
        // `r##` rather than `r#`: the JSON contains `"#FFFFFF"`, and the `"#`
        // sequence would close a single-hash raw string early.
        let json = r##"{
            "format_version": 1,
            "stage": { "width": 550, "height": 400, "background": "#FFFFFF", "frame_rate": 24 },
            "layers": [
                { "id": 1, "name": "Layer_1", "kind": "Normal", "visible": true,
                  "locked": false, "color": "#0099FF" }
            ],
            "max_id": 1
        }"##;
        let dto: DocumentDto = serde_json::from_str(json).unwrap();
        let scene = dto.to_scene().unwrap();
        assert_eq!(scene.layers().len(), 1);
        assert_eq!(scene.shape_count(), 0);
    }

    // -- rigging, version 6 --------------------------------------------------

    /// A rigged arm, bent, with a joint limit and a pin — every field the
    /// format gained in version 6.
    fn rigged_scene() -> Scene {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Character", LayerKind::Normal);

        let mut armature = buzz_rig::Armature::new(Point::new(100.0, 100.0));
        armature.push(buzz_rig::Bone::new("upper", None, 50.0, 0.2));
        armature.push(buzz_rig::Bone::new("fore", Some(0), 40.0, -0.4).with_limits(-1.2, 0.1));
        armature.push(buzz_rig::Bone::new("hand", Some(1), 15.0, 0.1).pinned());

        let mut rig = buzz_scene::ArmatureData::new(armature);
        // Two named poses — version 19.
        rig.poses.push(buzz_scene::NamedPose {
            name: "Reach".into(),
            angles: vec![0.4, -0.9, 0.1],
        });
        rig.poses.push(buzz_scene::NamedPose {
            name: "Rest".into(),
            angles: vec![0.0, 0.0, 0.0],
        });
        rig.bind_shape(Arc::new(Object::shape(
            ObjectId(50),
            ShapeData::filled(
                Rect::new(100.0, 92.0, 205.0, 108.0).to_path(1e-9),
                Color::from_rgb8(0x88, 0x44, 0x22),
            ),
        )));
        rig.bind_rigid(
            Arc::new(Object::shape(
                ObjectId(51),
                ShapeData::filled(
                    Rect::new(190.0, 90.0, 210.0, 110.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )),
            2,
        );
        // The pattern it was assembled from — version 26.
        rig.pattern = Some("Prop".into());

        scene.add_object(
            layer,
            Object {
                id: ObjectId(60),
                name: Some("Arm".into()),
                transform: Affine::translate((5.0, 5.0)),
                kind: ObjectKind::Armature(rig),
                locked: false,
                visible: true,
                filters: Vec::new(),
                blend: Default::default(),
                spatial: Default::default(),
                pivot: None,
                modifiers: Vec::new(),
                text: None,
                reverse: None,
            },
        );
        scene
    }

    #[test]
    fn an_armature_survives_a_round_trip_with_its_pose_intact() {
        let scene = rigged_scene();
        let dto = DocumentDto::from_scene(&scene);
        assert_eq!(dto.format_version, FORMAT_VERSION);

        let json = serde_json::to_string(&dto).expect("serialise");
        let back: DocumentDto = serde_json::from_str(&json).expect("deserialise");
        let loaded = back.to_scene().expect("to scene");

        let object = loaded
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .next()
            .expect("the armature")
            .clone();

        let ObjectKind::Armature(rig) = &object.kind else {
            panic!("the armature came back as something else");
        };
        assert_eq!(rig.armature.len(), 3);
        assert_eq!(rig.parts.len(), 2);

        // The pose, which is the thing an animator would lose.
        assert!((rig.armature.bones[0].angle - 0.2).abs() < 1e-12);
        assert!((rig.armature.bones[1].angle - -0.4).abs() < 1e-12);

        // And the rig's own settings.
        let limits = rig.armature.bones[1].limits.expect("limits");
        assert!((limits.min - -1.2).abs() < 1e-12);
        assert!((limits.max - 0.1).abs() < 1e-12);
        assert!(rig.armature.bones[2].pinned, "the pin was lost");
        assert_eq!(rig.armature.bones[1].name, "fore");

        // The pose library, version 19. A character's poses are work in their
        // own right; losing them on save would be losing the animation.
        assert_eq!(rig.poses.len(), 2, "the saved poses are gone");
        assert_eq!(rig.poses[0].name, "Reach");
        assert!((rig.poses[0].angles[1] - -0.9).abs() < 1e-12);
        assert_eq!(rig.poses[1].name, "Rest");

        // The pattern, version 26: without it the Rigging panel reopens a
        // character as a plain list of bones and there is nowhere to drop a
        // redrawn arm.
        assert_eq!(rig.pattern.as_deref(), Some("Prop"));
    }

    /// **A rig from before version 26 loads, with no pattern.**
    ///
    /// Which is every rig built with the Bone tool, then and now: the field is
    /// `#[serde(default)]` and absent means "not assembled from a pattern".
    #[test]
    fn a_rig_from_before_patterns_still_loads() {
        let scene = rigged_scene();
        let dto = DocumentDto::from_scene(&scene);
        let mut json = serde_json::to_value(&dto).expect("serialise");

        // Remove the field entirely, exactly as a version-25 file has it.
        fn strip(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(map) => {
                    map.remove("pattern");
                    for (_, v) in map.iter_mut() {
                        strip(v);
                    }
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
                _ => {}
            }
        }
        strip(&mut json);

        let back: DocumentDto = serde_json::from_value(json).expect("deserialise");
        let loaded = back.to_scene().expect("to scene");
        let object = loaded
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .next()
            .expect("the armature")
            .clone();

        let ObjectKind::Armature(rig) = &object.kind else {
            panic!("expected an armature");
        };
        assert!(rig.pattern.is_none());
        assert_eq!(rig.armature.len(), 3, "the rest of the rig came through");
    }

    /// A rig that was never assembled from a pattern writes exactly what it
    /// wrote in version 25 — the field is skipped, not written as `null`.
    #[test]
    fn a_pattern_less_rig_writes_no_pattern_field() {
        let mut scene = rigged_scene();
        scene.update_object(ObjectId(60), |object| {
            if let ObjectKind::Armature(rig) = &mut object.kind {
                rig.pattern = None;
            }
        });

        let json = serde_json::to_string(&DocumentDto::from_scene(&scene)).expect("serialise");
        assert!(
            !json.contains("\"pattern\""),
            "an unpatterned rig wrote a pattern field"
        );
    }

    /// **A file from before version 19 loads, with no poses.**
    ///
    /// The field is `#[serde(default)]`, and this is what proves it: the same
    /// strip-the-field trick the other back-compat tests use.
    #[test]
    fn a_rig_from_before_poses_still_loads() {
        let scene = rigged_scene();
        let dto = DocumentDto::from_scene(&scene);
        let mut json = serde_json::to_value(&dto).expect("serialise");

        // Remove the field entirely, exactly as a version-18 file has it.
        fn strip(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(map) => {
                    map.remove("poses");
                    for (_, v) in map.iter_mut() {
                        strip(v);
                    }
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
                _ => {}
            }
        }
        strip(&mut json);

        let older: DocumentDto = serde_json::from_value(json).expect("read it back");
        let scene = older.to_scene().expect("build the scene");
        let object = scene.layers().iter().next().unwrap().objects_at(0)[0].clone();
        let ObjectKind::Armature(rig) = &object.kind else {
            panic!("the armature came back as something else");
        };
        assert!(rig.poses.is_empty(), "poses appeared from nowhere");
        // And the rest of the rig is untouched.
        assert_eq!(rig.armature.len(), 3);
        assert_eq!(rig.parts.len(), 2);
    }

    /// Weights are what make the artwork bend; losing them would leave a rig
    /// that moves its bones and not its drawing.
    #[test]
    fn skin_weights_and_rigid_attachments_both_survive() {
        let scene = rigged_scene();
        let json = serde_json::to_string(&DocumentDto::from_scene(&scene)).expect("serialise");
        let loaded: DocumentDto = serde_json::from_str(&json).expect("deserialise");
        let loaded = loaded.to_scene().expect("to scene");

        let object = loaded
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .next()
            .expect("the armature")
            .clone();
        let ObjectKind::Armature(rig) = &object.kind else {
            panic!("expected an armature");
        };

        match &rig.parts[0].binding {
            buzz_scene::RigBinding::Skin(skin) => {
                assert!(!skin.points.is_empty(), "the weights are gone");
                for weights in &skin.points {
                    let total: f64 = weights.iter().map(|(_, w)| w).sum();
                    assert!((total - 1.0).abs() < 1e-9, "weights no longer sum to one");
                }
            }
            other => panic!("the skinned part came back as {other:?}"),
        }
        assert_eq!(rig.parts[1].binding, buzz_scene::RigBinding::Rigid(2));
    }

    /// The deformed artwork is *derived*. Saving it would be a second copy of
    /// something the file can already work out, and the two would drift apart.
    #[test]
    fn the_deformed_artwork_is_not_stored_only_the_pose() {
        let scene = rigged_scene();
        let json = serde_json::to_string(&DocumentDto::from_scene(&scene)).expect("serialise");

        let value: serde_json::Value = serde_json::from_str(&json).expect("parse");
        let text = value.to_string();
        assert!(text.contains("\"rest_angle\""), "the pose should be there");
        assert!(
            !text.contains("\"posed\"") && !text.contains("\"deformed\""),
            "the file appears to store deformed artwork"
        );
    }

    #[test]
    fn a_warp_survives_a_round_trip_with_its_handles() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Cloth", LayerKind::Normal);

        let shape = ShapeData::filled(
            Rect::new(0.0, 0.0, 120.0, 80.0).to_path(1e-9),
            Color::from_rgb8(0x22, 0x88, 0x44),
        );
        let mut warp = buzz_scene::WarpData::new(shape).with_grid(3, 3);
        warp.handles[4].current = Point::new(70.0, 10.0);
        warp.rigidity = 1.5;

        scene.add_object(
            layer,
            Object {
                id: ObjectId(9),
                name: None,
                transform: Affine::IDENTITY,
                kind: ObjectKind::Warp(warp),
                locked: false,
                visible: true,
                filters: Vec::new(),
                blend: Default::default(),
                spatial: Default::default(),
                pivot: None,
                modifiers: Vec::new(),
                text: None,
                reverse: None,
            },
        );

        let json = serde_json::to_string(&DocumentDto::from_scene(&scene)).expect("serialise");
        let loaded: DocumentDto = serde_json::from_str(&json).expect("deserialise");
        let loaded = loaded.to_scene().expect("to scene");

        let object = loaded
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .next()
            .expect("the warp")
            .clone();
        let ObjectKind::Warp(warp) = &object.kind else {
            panic!("expected a warp");
        };

        assert_eq!(warp.handles.len(), 9);
        assert!((warp.rigidity - 1.5).abs() < 1e-12);
        assert!((warp.handles[4].current.x - 70.0).abs() < 1e-12);
        assert_ne!(
            warp.handles[4].current, warp.handles[4].rest,
            "the dragged handle came back where it started"
        );
    }

    /// A file naming a parent bone that comes later would be a cycle, and the
    /// solver would walk it forever. Loading has to neutralise that.
    #[test]
    fn a_file_describing_a_cyclic_skeleton_loads_safely() {
        let json = r##"{
            "format_version": 6,
            "stage": { "width": 550, "height": 400, "background": "#FFFFFF", "frame_rate": 24 },
            "layers": [
                { "id": 1, "name": "Layer_1", "kind": "Normal", "visible": true,
                  "locked": false, "color": "#0099FF",
                  "keyframes": [ { "start": 0, "objects": [
                    { "id": 2, "transform": [1,0,0,1,0,0], "visible": true, "locked": false,
                      "kind": { "type": "armature", "root": [0, 0], "parts": [],
                        "bones": [
                          { "name": "a", "parent": 1, "length": 10, "rest_angle": 0, "angle": 0 },
                          { "name": "b", "parent": 0, "length": 10, "rest_angle": 0, "angle": 0 }
                        ] } }
                  ] } ] }
            ],
            "max_id": 2
        }"##;

        let dto: DocumentDto = serde_json::from_str(json).expect("parse");
        let scene = dto.to_scene().expect("load");
        let object = scene
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter())
            .next()
            .expect("the armature")
            .clone();
        let ObjectKind::Armature(rig) = &object.kind else {
            panic!("expected an armature");
        };

        assert_eq!(
            rig.armature.bones[0].parent, None,
            "the cycle was not broken"
        );
        // And it resolves rather than hanging.
        assert_eq!(rig.armature.joints().len(), 2);
    }
}

#[cfg(test)]
mod layer_alpha_tests {
    use super::*;
    use buzz_scene::{LayerKind, Scene};

    /// A dimmed layer stays dimmed across a save.
    #[test]
    fn layer_transparency_survives_the_round_trip() {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Reference", LayerKind::Normal);
        scene.update_layer(layer, |l| l.alpha = 0.35);

        let back = DocumentDto::from_scene(&scene)
            .to_scene()
            .expect("round trip");
        let after = back.layers().get(layer).expect("the layer came back");
        assert!(
            (after.alpha - 0.35).abs() < 1e-9,
            "transparency was lost: {}",
            after.alpha
        );
    }

    /// **A document written before this existed opens fully solid.**
    ///
    /// The failure it prevents is silent and total: a missing field defaulting
    /// to zero would open every layer of every older film completely invisible.
    #[test]
    fn a_layer_from_an_older_document_is_fully_solid() {
        let mut scene = Scene::default();
        scene.add_layer("Old", LayerKind::Normal);
        let dto = DocumentDto::from_scene(&scene);

        let mut json = serde_json::to_value(&dto).expect("serialise");
        // Strip the field, exactly as a file from before version 18 has it.
        for layer in json["layers"].as_array_mut().expect("layers") {
            layer.as_object_mut().expect("a layer").remove("alpha");
        }

        let older: DocumentDto = serde_json::from_value(json).expect("read it back");
        let back = older.to_scene().expect("to scene");
        for layer in back.layers().iter() {
            assert_eq!(
                layer.alpha, 1.0,
                "a layer with no transparency recorded must open solid"
            );
        }
    }

    #[test]
    fn the_compositor_look_survives_the_round_trip() {
        let mut scene = Scene::default();
        scene.stage_mut().post = buzz_scene::PostSettings {
            enabled: true,
            bloom: buzz_scene::BloomSettings {
                enabled: true,
                threshold: 0.6,
                intensity: 0.7,
                radius: 0.4,
            },
            grade: buzz_scene::GradeSettings {
                enabled: true,
                exposure: 0.5,
                contrast: 1.2,
                saturation: 0.8,
                temperature: 0.1,
                tint: -0.2,
                lift: 0.05,
                gamma: 0.9,
                gain: 1.1,
            },
            posterise: buzz_scene::PosteriseSettings { enabled: true, levels: 4 },
            halftone: buzz_scene::HalftoneSettings { enabled: true, scale: 10.0 },
            hatching: buzz_scene::HatchingSettings { enabled: true, scale: 8.0 },
            ..Default::default()
        };

        let back = DocumentDto::from_scene(&scene)
            .to_scene()
            .expect("round trip");
        assert_eq!(back.stage().post, scene.stage().post, "the look changed");
    }

    /// **A document written before the compositor opens with effects off.**
    ///
    /// The failure it guards against is a document from version 19 or earlier
    /// suddenly rendering through a bloomed, graded pipeline it never asked for.
    #[test]
    fn a_document_from_before_the_compositor_has_no_effects() {
        let mut scene = Scene::default();
        scene.stage_mut().post.enabled = true;
        scene.stage_mut().post.vignette.enabled = true;
        let dto = DocumentDto::from_scene(&scene);

        let mut json = serde_json::to_value(&dto).expect("serialise");
        // Strip the field, exactly as a file from before version 20 has it.
        json["stage"]
            .as_object_mut()
            .expect("a stage")
            .remove("post");

        let older: DocumentDto = serde_json::from_value(json).expect("read it back");
        let back = older.to_scene().expect("to scene");
        assert_eq!(
            back.stage().post,
            buzz_scene::PostSettings::default(),
            "a document with no post settings recorded must open with effects off"
        );
        assert!(back.stage().post.is_identity());
    }

    /// A document whose look is untouched writes no `post` block at all, so it
    /// stays byte-identical to what version 19 produced.
    #[test]
    fn an_unmodified_look_writes_no_post_block() {
        let scene = Scene::default();
        let json = serde_json::to_value(DocumentDto::from_scene(&scene)).expect("serialise");
        assert!(
            json["stage"].as_object().expect("stage").get("post").is_none(),
            "a default look must not write a post block"
        );
    }

    /// **A fire survives a save.**
    ///
    /// The gutter is one number and it is what makes the light a fire, so a
    /// document that opens without it opens with an ordinary orange lamp — a
    /// failure that looks exactly like the animator having imagined it.
    #[test]
    fn a_fire_survives_the_round_trip() {
        let mut scene = Scene::default();
        let mut lamp = buzz_scene::Light::new(
            buzz_scene::LightId(7),
            "Hearth",
            buzz_scene::LightKind::lamp(buzz_geom::Point::new(100.0, 200.0)),
        );
        lamp.make_fire();
        let expected = lamp.clone();
        scene.lights_mut().enabled = true;
        scene.lights_mut().lights.push(lamp);

        let back = DocumentDto::from_scene(&scene)
            .to_scene()
            .expect("round trip");
        assert_eq!(back.lights().lights[0], expected, "the fire went out on save");
        assert!(back.lights().animates(), "and it must still animate");
    }

    /// A light that does not gutter must not start writing a `flicker`, so a
    /// document with no fire in it is byte-identical to what the version before
    /// this wrote.
    #[test]
    fn a_steady_light_writes_no_flicker() {
        let mut scene = Scene::default();
        scene.add_light(buzz_scene::LightKind::sun());
        let json = serde_json::to_value(DocumentDto::from_scene(&scene)).expect("serialise");
        let light = &json["lights"]["lights"][0];
        assert!(
            light.as_object().expect("a light").get("flicker").is_none(),
            "a steady light wrote a flicker: {light}"
        );
    }

    /// **A gloom survives a save**, wall, bearing, throw and width.
    ///
    /// It is written through the same flat fields a lamp and a sun use, which
    /// is the arrangement that lets an older build read the file at all — and
    /// the arrangement that would silently drop the two numbers that are only
    /// a gloom's if either write path forgot them. There are two write paths,
    /// one for the light and one for its keyframes, so both are checked here.
    #[test]
    fn a_gloom_survives_the_round_trip() {
        let mut scene = Scene::default();
        let kind = buzz_scene::LightKind::Gloom {
            edge: buzz_geom::Point::new(-320.5, 77.25),
            facing: 0.85,
            throw: 1234.0,
            width: 3210.0,
        };
        let mut dark = buzz_scene::Light::new(buzz_scene::LightId(4), "Dark", kind);

        let mut track = buzz_scene::LightTrack::new();
        track.enabled = true;
        track.set_key(buzz_scene::LightKey {
            frame: 0,
            color: peniko::Color::from_rgb8(0x0B, 0x0E, 0x18),
            intensity: 1.0,
            softness: 0.35,
            standing_height: 70.0,
            shadow_strength: 0.45,
            kind,
        });
        track.set_key(buzz_scene::LightKey {
            frame: 48,
            color: peniko::Color::from_rgb8(0x0B, 0x0E, 0x18),
            intensity: 0.4,
            softness: 0.35,
            standing_height: 70.0,
            shadow_strength: 0.45,
            kind: buzz_scene::LightKind::Gloom {
                edge: buzz_geom::Point::new(120.0, 77.25),
                facing: 0.85,
                throw: 600.0,
                width: 3210.0,
            },
        });
        dark.track = Some(track);

        scene.lights_mut().enabled = true;
        scene.lights_mut().lights.push(dark);

        let back = DocumentDto::from_scene(&scene)
            .to_scene()
            .expect("round trip");
        let restored = &back.lights().lights[0];
        assert_eq!(restored.kind, kind, "the wall moved across a save");
        assert_eq!(
            restored.track,
            scene.lights().lights[0].track,
            "the gloom's keyframes changed across a save"
        );
    }

    #[test]
    fn a_light_track_survives_the_round_trip() {
        let mut scene = Scene::default();
        let mut sun = buzz_scene::Light::new(
            buzz_scene::LightId(1),
            "Key",
            buzz_scene::LightKind::Sun {
                azimuth: 0.2,
                elevation: 0.3,
            },
        );
        let mut track = buzz_scene::LightTrack::new();
        track.enabled = true;
        track.set_key(buzz_scene::LightKey {
            frame: 0,
            color: peniko::Color::WHITE,
            intensity: 1.0,
            softness: 0.35,
            standing_height: 40.0,
            shadow_strength: 0.45,
            kind: buzz_scene::LightKind::Sun {
                azimuth: 0.2,
                elevation: 0.3,
            },
        });
        track.set_key(buzz_scene::LightKey {
            frame: 24,
            color: peniko::Color::from_rgb8(0xFF, 0x80, 0x00),
            intensity: 2.0,
            softness: 0.5,
            standing_height: 60.0,
            shadow_strength: 0.6,
            kind: buzz_scene::LightKind::Sun {
                azimuth: 1.0,
                elevation: 1.2,
            },
        });
        sun.track = Some(track);
        scene.lights_mut().enabled = true;
        scene.lights_mut().lights.push(sun);

        let back = DocumentDto::from_scene(&scene)
            .to_scene()
            .expect("round trip");
        let restored = &back.lights().lights[0];
        assert_eq!(
            restored.track,
            scene.lights().lights[0].track,
            "the light track changed across a save"
        );
    }

    /// A document from before keyframed lights loads with static lights, so an
    /// older film is not suddenly animated.
    #[test]
    fn a_light_from_before_tracks_is_static() {
        let mut scene = Scene::default();
        let sun = buzz_scene::Light::new(
            buzz_scene::LightId(1),
            "Key",
            buzz_scene::LightKind::sun(),
        );
        scene.lights_mut().enabled = true;
        scene.lights_mut().lights.push(sun);

        let mut json = serde_json::to_value(DocumentDto::from_scene(&scene)).expect("serialise");
        // A version-20 file simply has no `track` on any light.
        if let Some(lights) = json["lights"]["lights"].as_array_mut() {
            for light in lights {
                light.as_object_mut().expect("a light").remove("track");
            }
        }
        let older: DocumentDto = serde_json::from_value(json).expect("read back");
        let back = older.to_scene().expect("to scene");
        assert!(
            back.lights().lights[0].track.is_none(),
            "a light with no track recorded must open static"
        );
    }

    #[test]
    fn depth_sorting_survives_the_round_trip_and_defaults_off() {
        let scene = Scene::default();
        // Off by default and not written, so an untouched document is unchanged.
        let json = serde_json::to_value(DocumentDto::from_scene(&scene)).expect("serialise");
        assert!(
            json["stage"]
                .as_object()
                .expect("stage")
                .get("sort_by_depth")
                .is_none(),
            "a default document must not write sort_by_depth"
        );

        let mut on = Scene::default();
        on.stage_mut().sort_by_depth = true;
        let back = DocumentDto::from_scene(&on).to_scene().expect("round trip");
        assert!(back.stage().sort_by_depth);
    }

    #[test]
    fn camera_aperture_and_angles_survive_the_round_trip() {
        let mut scene = Scene::default();
        {
            let cam = scene.camera_mut();
            cam.enabled = true;
            cam.aperture = 0.03;
            cam.focus_depth = 250.0;
            cam.set_key(buzz_scene::CameraKey::new(0, buzz_geom::Point::new(10.0, 20.0)));
            cam.save_angle("Wide", 0);
            cam.save_angle("Close", 0);
        }

        let back = DocumentDto::from_scene(&scene)
            .to_scene()
            .expect("round trip");
        assert!((back.camera().aperture - 0.03).abs() < 1e-9);
        assert!((back.camera().focus_depth - 250.0).abs() < 1e-9);
        assert_eq!(back.camera().angles.len(), 2);
        assert_eq!(back.camera().angle("Close").map(|a| a.name.as_str()), Some("Close"));
    }

    /// A nonsense value in a file cannot make a layer invisible.
    #[test]
    fn a_corrupt_transparency_is_clamped_rather_than_believed() {
        let mut scene = Scene::default();
        scene.add_layer("Odd", LayerKind::Normal);
        let dto = DocumentDto::from_scene(&scene);

        for (written, want) in [(-4.0, 0.0), (17.0, 1.0), (f64::NAN, 1.0)] {
            let mut json = serde_json::to_value(&dto).expect("serialise");
            for layer in json["layers"].as_array_mut().expect("layers") {
                let value = serde_json::Number::from_f64(written)
                    .map(serde_json::Value::Number)
                    // NaN has no JSON spelling; a null exercises the default.
                    .unwrap_or(serde_json::Value::Null);
                layer
                    .as_object_mut()
                    .expect("a layer")
                    .insert("alpha".into(), value);
            }
            let Ok(read): Result<DocumentDto, _> = serde_json::from_value(json) else {
                continue;
            };
            let back = read.to_scene().expect("to scene");
            for layer in back.layers().iter() {
                assert_eq!(layer.alpha, want, "{written} should have been clamped");
            }
        }
    }
}
