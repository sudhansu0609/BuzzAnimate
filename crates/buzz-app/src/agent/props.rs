//! Stage D of `docs/AI_AGENT_PLAN.md`: `set_property {target, field, value}` and
//! `inspect {what: "properties", target}`.
//!
//! Every field a person can edit in the Properties, Layers, Timeline, Camera,
//! Lighting, Effects, Layer Depth, Sound and Rigging panels, reached through the
//! same mutation the panel makes, inside one `doc.edit` carrying the panel's own
//! undo label. A call is validated on a copy of the scene first, so a bad
//! target, field or value changes nothing.
//!
//! Targets: `element:<id>` | `layer:<index|name>` | `frame:<layer>:<frame>` |
//! `light:<id>` | `light:rig` | `camera` | `document` | `scene`.

use super::{AgentResult, hex_of, parse_hex};
use crate::app::App;
use buzz_scene::{
    Blend, CameraKey, ColorEffect, ColorTransform, EdgeMode, Easing, EditAt, LayerId, LayerKind,
    LightId, LightKind, LoopMode, ObjectId, ObjectKind, Scene, ShadeModel, ShadowFall, SoundRef,
    SoundSync, TextAlign, TweenKind,
};
use peniko::Color;
use serde_json::{Value, json};
use std::fmt::Debug;

/// `(target kind, field, value hint, panel)`: every property the agent can set.
/// The kind is the target's prefix (`light_rig` is the target `light:rig`). A
/// field with `<n>` in it is one per bone of an armature element.
pub const PROPERTY_FIELDS: &[(&str, &str, &str, &str)] = &[
    // -- Properties panel, nothing selected: Document ------------------------
    ("document", "width", "number 1..16384", "Properties/Document"),
    ("document", "height", "number 1..16384", "Properties/Document"),
    ("document", "fps", "number 0.01..240", "Properties/Document"),
    ("document", "background", "colour #rrggbb[aa]", "Properties/Document"),
    ("document", "sort_by_depth", "bool", "Properties/Document"),
    ("document", "frame_count", "integer 1..16000, extends or trims every layer", "Timeline"),
    ("document", "loop_enabled", "bool", "Timeline"),
    ("document", "loop_start", "integer frame, 0-based", "Timeline"),
    ("document", "loop_end", "integer frame, 0-based", "Timeline"),
    ("document", "loop_repeats", "integer 1..", "Timeline"),
    // -- Effects (full-frame look) --------------------------------------------
    ("document", "post.enabled", "bool", "Properties/Effects"),
    ("document", "post.bloom.enabled", "bool", "Properties/Effects"),
    ("document", "post.bloom.threshold", "number 0..1", "Properties/Effects"),
    ("document", "post.bloom.intensity", "number 0..3", "Properties/Effects"),
    ("document", "post.bloom.radius", "number 0..1", "Properties/Effects"),
    ("document", "post.grade.enabled", "bool", "Properties/Effects"),
    ("document", "post.grade.exposure", "number -3..3", "Properties/Effects"),
    ("document", "post.grade.contrast", "number 0..2", "Properties/Effects"),
    ("document", "post.grade.saturation", "number 0..2", "Properties/Effects"),
    ("document", "post.grade.temperature", "number -1..1", "Properties/Effects"),
    ("document", "post.grade.tint", "number -1..1", "Properties/Effects"),
    ("document", "post.grade.lift", "number -0.5..0.5", "Properties/Effects"),
    ("document", "post.grade.gamma", "number 0.1..3", "Properties/Effects"),
    ("document", "post.grade.gain", "number 0..2", "Properties/Effects"),
    ("document", "post.vignette.enabled", "bool", "Properties/Effects"),
    ("document", "post.vignette.amount", "number 0..1", "Properties/Effects"),
    ("document", "post.vignette.softness", "number 0..1", "Properties/Effects"),
    ("document", "post.vignette.color", "colour #rrggbb[aa]", "Properties/Effects"),
    ("document", "post.grain.enabled", "bool", "Properties/Effects"),
    ("document", "post.grain.amount", "number 0..1", "Properties/Effects"),
    ("document", "post.grain.size", "number 1..8", "Properties/Effects"),
    ("document", "post.posterise.enabled", "bool", "Properties/Effects"),
    ("document", "post.posterise.levels", "integer 2..16", "Properties/Effects"),
    ("document", "post.halftone.enabled", "bool", "Properties/Effects"),
    ("document", "post.halftone.scale", "number 2..24", "Properties/Effects"),
    ("document", "post.hatching.enabled", "bool", "Properties/Effects"),
    ("document", "post.hatching.scale", "number 2..24", "Properties/Effects"),
    // -- Scene ---------------------------------------------------------------
    ("scene", "name", "text, unique, not undoable (as in the scene tabs)", "Scene tabs"),
    // -- Camera, Depth of Field, Motion Blur, Layer Depth --------------------
    ("camera", "enabled", "bool", "Camera"),
    ("camera", "locked", "bool", "Timeline"),
    ("camera", "zoom", "number 0.1..8, at the playhead", "Camera"),
    ("camera", "rotation", "degrees -180..180, at the playhead", "Camera"),
    ("camera", "pitch", "degrees about -74..74, at the playhead", "Camera"),
    ("camera", "yaw", "degrees about -74..74, at the playhead", "Camera"),
    ("camera", "focal_distance", "number 200..6000 (camera depth)", "Layer Depth"),
    ("camera", "aperture", "number 0..0.2 (0 is a pinhole)", "Properties/Depth of Field"),
    ("camera", "focus_depth", "number, the layer depth kept sharp", "Properties/Depth of Field"),
    ("camera", "shutter", "number 0..1 frames", "Properties/Motion Blur"),
    ("camera", "blur_samples", "integer 2..64", "Properties/Motion Blur"),
    // -- Layers, Timeline, Effects, Layer Depth ------------------------------
    ("layer", "name", "text", "Layers"),
    ("layer", "visible", "bool", "Layers"),
    ("layer", "locked", "bool", "Layers"),
    ("layer", "outline", "bool", "Layers"),
    ("layer", "alpha", "percent 0..100", "Layers"),
    ("layer", "kind", "normal|mask|inverse_mask|masked|guide|guided", "Layers"),
    ("layer", "follows", "layer name or index, or null", "Layers"),
    ("layer", "highlight", "colour #rrggbb[aa], or null", "Layers"),
    ("layer", "unlit", "bool (Ignore lights: no shading, no cast shadow)", "Layers"),
    ("layer", "casts_shadow", "bool (Cast shadow: off = still lit, no cast shadow)", "Layers"),
    ("layer", "collapsed", "bool", "Layers"),
    ("layer", "depth", "number", "Layer Depth"),
    ("layer", "blend", "blend mode name, snake_case", "Effects"),
    ("layer", "brightness", "percent -100..100", "Effects"),
    ("layer", "tint", "colour #rrggbb", "Effects"),
    ("layer", "tint_amount", "percent 0..100", "Effects"),
    // -- Timeline keyframes, Motion Editor, Sound ------------------------------
    ("frame", "label", "text, or null", "Timeline"),
    ("frame", "tween", "none|classic|motion|shape (needs a later keyframe)", "Timeline"),
    ("frame", "ease", "\"linear\", a strength -100..100, or [x1,y1,x2,y2]", "Motion Editor"),
    ("frame", "sound", "name of a library sound, or null", "Sound"),
    ("frame", "sound_sync", "event|start|stop|stream", "Sound"),
    ("frame", "sound_volume", "number 0..1", "Sound"),
    ("frame", "sound_loops", "integer 0..999 (0 loops for ever)", "Sound"),
    ("frame", "sound_trim_start", "seconds", "Sound"),
    ("frame", "sound_trim_end", "seconds, or null for the clip's end", "Sound"),
    ("frame", "sound_start", "integer frame, 0-based: moves the keyframe carrying the sound there (Move Sound); the target then needs the new frame", "Sound"),
    // -- Properties panel, an element is selected --------------------------------
    ("element", "blend", "blend mode name, snake_case", "Effects"),
    ("element", "rotate_x", "degrees -85..85", "Properties/3D"),
    ("element", "rotate_y", "degrees -85..85", "Properties/3D"),
    ("element", "rotate_z", "degrees -85..85", "Properties/3D"),
    ("element", "z", "number -800..800", "Properties/3D"),
    ("element", "symbol", "library symbol name (swap)", "Properties/Instance"),
    ("element", "first_frame", "integer, 1-based", "Properties/Instance"),
    ("element", "loop_mode", "loop|play_once|single_frame (graphics only)", "Properties/Instance"),
    ("element", "sync_to_root", "bool", "Properties/Instance"),
    ("element", "color_effect", "none|brightness|tint|alpha", "Properties/Instance"),
    ("element", "brightness", "percent -100..100 (colour effect brightness)", "Properties/Instance"),
    ("element", "alpha", "percent 0..100 (colour effect alpha)", "Properties/Instance"),
    ("element", "tint", "colour #rrggbb (colour effect tint)", "Properties/Instance"),
    ("element", "tint_amount", "percent 0..100 (colour effect tint)", "Properties/Instance"),
    ("element", "symbol_set", "read-only: names of the symbols in this instance's set, owner first ([] when none)", "Properties/Instance"),
    ("element", "similar_symbols", "read-only: names of the symbols Swap… lists as similar to the one shown, ranked (set members first)", "Properties/Instance"),
    ("element", "pick_from_set", "set member name, or {\"symbol\": name, \"start\": \"same_frame\"|\"from_start\"}: swap from the playhead on (reads the symbol shown there)", "Properties/Instance"),
    ("element", "include_in_set", "library symbol name to include in this instance's set (reads the names it would accept)", "Properties/Instance"),
    ("element", "remove_from_set", "member name to take out of the set (reads the names it would accept)", "Properties/Instance"),
    ("element", "text", "string", "Properties/Text"),
    ("element", "text_size", "number 4..400", "Properties/Text"),
    ("element", "text_font", "family name, or null for the default", "Properties/Text"),
    ("element", "text_bold", "bool", "Properties/Text"),
    ("element", "text_italic", "bool", "Properties/Text"),
    ("element", "text_align", "left|centre|right", "Properties/Text"),
    ("element", "bone.<n>.name", "text", "Rigging"),
    ("element", "bone.<n>.pinned", "bool", "Rigging"),
    ("element", "bone.<n>.limited", "bool (true starts at -90..90 degrees)", "Rigging"),
    ("element", "bone.<n>.min_angle", "degrees -180..180 (needs limited)", "Rigging"),
    ("element", "bone.<n>.max_angle", "degrees -180..180 (needs limited)", "Rigging"),
    ("element", "bone.<n>.angle", "degrees -360..360, the pose angle relative to the parent bone (clamped to its limits); written on the playhead's keyframe, a new one when auto_keyframe is on", "Rigging"),
    // -- Lighting: one light -----------------------------------------------------
    ("light", "name", "text", "Lighting"),
    ("light", "enabled", "bool", "Lighting"),
    ("light", "color", "colour #rrggbb", "Lighting"),
    ("light", "intensity", "number 0..4 (0..1 for a gloom)", "Lighting"),
    ("light", "softness", "number 0..1", "Lighting"),
    ("light", "glow", "number 0..1", "Lighting"),
    ("light", "flicker", "number 0..1", "Lighting"),
    ("light", "rim", "number 0..1", "Lighting"),
    ("light", "glint", "number 0..1", "Lighting"),
    ("light", "storm", "number 0..1", "Lighting"),
    ("light", "shadows", "bool", "Lighting"),
    ("light", "fall", "ground|wall", "Lighting"),
    ("light", "shadow_strength", "number 0..1", "Lighting"),
    ("light", "shadow_length", "number 0..3", "Lighting"),
    ("light", "standing_height", "number 0..400", "Lighting"),
    ("light", "shadow_offset_x", "number", "Lighting"),
    ("light", "shadow_offset_y", "number", "Lighting"),
    ("light", "shadow_scale_x", "number 0.01..100", "Lighting"),
    ("light", "shadow_scale_y", "number 0.01..100", "Lighting"),
    ("light", "shadow_rotation", "degrees", "Lighting"),
    ("light", "shadow_skew_x", "degrees -80..80", "Lighting"),
    ("light", "shadow_skew_y", "degrees -80..80", "Lighting"),
    ("light", "shadow_pivot_x", "number", "Lighting"),
    ("light", "shadow_pivot_y", "number", "Lighting"),
    ("light", "azimuth", "degrees -180..180 (sun)", "Lighting"),
    ("light", "elevation", "degrees 0..90 (sun)", "Lighting"),
    ("light", "horizon", "colour #rrggbb (sky)", "Lighting"),
    ("light", "x", "number (lamp)", "Lighting"),
    ("light", "y", "number (lamp)", "Lighting"),
    ("light", "height", "number 4..1200 (lamp)", "Lighting"),
    ("light", "radius", "number 40..3000 (lamp)", "Lighting"),
    ("light", "edge_x", "number (gloom)", "Lighting"),
    ("light", "edge_y", "number (gloom)", "Lighting"),
    ("light", "facing", "degrees -180..180 (gloom)", "Lighting"),
    ("light", "throw", "number 100..6000 (gloom)", "Lighting"),
    ("light", "width", "number 100..8000 (gloom)", "Lighting"),
    ("light", "placement", "stage-layer index the light sits directly above (it lights that layer and those below), or null for the top of the stack", "Lighting"),
    // -- Lighting: the rig ---------------------------------------------------------
    ("light_rig", "enabled", "bool", "Lighting"),
    ("light_rig", "base", "colour #rrggbb (fill light)", "Lighting"),
    ("light_rig", "modelling", "number 0..1", "Lighting"),
    ("light_rig", "edges", "off|shapes|figure", "Lighting"),
    ("light_rig", "model", "crescent|form", "Lighting"),
];

/// What the panels edit that `set_property` does not reach, and why. Short, and
/// every entry is not part of the document or has its own tool. The
/// list-structured edits (filters, modifiers, swatches, poses, bones, rest
/// pose, mirror) are the `edit_list` tool, and `frame.sound_start` and
/// `light.placement` are fields above.
pub const PROPERTY_EXCLUDED: &[(&str, &str)] = &[
    ("view.*", "editor view state (rulers, grid, guides, snapping): no document effect; the toggle_* commands change them"),
    ("tool_options.*", "drawing style and tool settings are the set_style tool"),
    ("timeline.view", "frame width, row scale, onion range, parenting and depth views, panel layout: workspace state saved with the layout, not the film"),
    ("cast.*", "Cast panel edits asset files on disk, not document fields"),
    ("align.*", "Align, Distribute, Match Size and Nudge are the align, distribute, match_size and nudge commands"),
];

// -- targets -------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Target {
    Element(ObjectId),
    Layer(LayerId),
    Frame(LayerId, u32),
    Light(LightId),
    LightRig,
    Camera,
    Document,
    Scene,
}

impl Target {
    /// The kind used in [`PROPERTY_FIELDS`].
    fn kind(&self) -> &'static str {
        match self {
            Target::Element(_) => "element",
            Target::Layer(_) => "layer",
            Target::Frame(..) => "frame",
            Target::Light(_) => "light",
            Target::LightRig => "light_rig",
            Target::Camera => "camera",
            Target::Document => "document",
            Target::Scene => "scene",
        }
    }
}

const TARGET_FORMS: &str =
    "element:<id> | layer:<index|name> | frame:<layer>:<frame> | light:<id> | light:rig | camera | document | scene";

/// What an edit needs to know about the editor.
struct Ctx {
    frame: u32,
    at: EditAt,
    auto_key: bool,
    scene_index: usize,
    scene_names: Vec<String>,
    /// Master Audio mode: `Editor::master_audio_lock` — `None` while off.
    master_lock: Option<(Option<std::sync::Arc<Vec<u8>>>, String)>,
}

pub(super) fn layer_by_ref(scene: &Scene, text: &str) -> Result<LayerId, String> {
    let layers: Vec<_> = scene.layers().iter().collect();
    if let Some(l) = layers.iter().find(|l| l.name == text) {
        return Ok(l.id);
    }
    if let Ok(i) = text.trim().parse::<usize>() {
        if let Some(l) = layers.get(i) {
            return Ok(l.id);
        }
    }
    let names: Vec<String> = layers
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{i}={}", l.name))
        .collect();
    Err(format!("no layer '{text}'; layers: {}", names.join(", ")))
}

pub(super) fn parse_target(scene: &Scene, text: &str) -> Result<Target, String> {
    let text = text.trim();
    match text {
        "camera" => return Ok(Target::Camera),
        "document" => return Ok(Target::Document),
        "scene" => return Ok(Target::Scene),
        "light:rig" => return Ok(Target::LightRig),
        _ => {}
    }
    let bad = || format!("unknown target '{text}'; valid: {TARGET_FORMS}");
    let (head, rest) = text.split_once(':').ok_or_else(bad)?;
    match head {
        "element" => {
            let id = ObjectId(rest.trim().parse::<u64>().map_err(|_| bad())?);
            if scene.find_object(id).is_none() {
                return Err(format!("no element {} in this scene", id.0));
            }
            Ok(Target::Element(id))
        }
        "layer" => Ok(Target::Layer(layer_by_ref(scene, rest.trim())?)),
        "frame" => {
            let (layer, frame) = rest.rsplit_once(':').ok_or_else(bad)?;
            let layer = layer_by_ref(scene, layer.trim())?;
            let frame = frame.trim().parse::<u32>().map_err(|_| bad())?;
            let length = scene.layers().get(layer).map(|l| l.frames.length()).unwrap_or(0);
            if frame >= length {
                return Err(format!("frame {frame} is past the end of that layer ({length} frames)"));
            }
            Ok(Target::Frame(layer, frame))
        }
        "light" => {
            let id = LightId(rest.trim().parse::<u64>().map_err(|_| bad())?);
            if scene.lights().get(id).is_none() {
                let ids: Vec<String> = scene.lights().lights.iter().map(|l| l.id.0.to_string()).collect();
                return Err(format!("no light {}; lights: {}", id.0, ids.join(", ")));
            }
            Ok(Target::Light(id))
        }
        _ => Err(bad()),
    }
}

// -- values ----------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Op<'a> {
    Get,
    Set(&'a Value),
}

impl Op<'_> {
    fn is_set(&self) -> bool {
        matches!(self, Op::Set(_))
    }
}

/// Lower `CamelCase` to `snake_case`.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn name_of<T: Debug>(value: T) -> Value {
    json!(snake(&format!("{value:?}")))
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn as_number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        .filter(|n| n.is_finite())
}

fn num(v: &Value, field: &str, min: f64, max: f64) -> Result<f64, String> {
    let n = as_number(v).ok_or_else(|| format!("'{field}' needs a number between {min} and {max}"))?;
    if n < min || n > max {
        return Err(format!("'{field}' must be between {min} and {max}, not {n}"));
    }
    Ok(n)
}

fn boolean(v: &Value, field: &str) -> Result<bool, String> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::String(s) if s == "true" => Ok(true),
        Value::String(s) if s == "false" => Ok(false),
        _ => Err(format!("'{field}' needs true or false")),
    }
}

fn colour(v: &Value, field: &str) -> Result<Color, String> {
    v.as_str()
        .and_then(parse_hex)
        .ok_or_else(|| format!("'{field}' needs a colour like \"#rrggbb\" or \"#rrggbbaa\""))
}

fn is_none_value(v: &Value) -> bool {
    v.is_null() || v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("none") || s.is_empty())
}

fn pick<T: Copy + Debug>(all: &[T], v: &Value, field: &str) -> Result<T, String> {
    let want: String = v
        .as_str()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    all.iter()
        .copied()
        .find(|t| {
            let have: String = snake(&format!("{t:?}")).chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            have == want
        })
        .ok_or_else(|| {
            let valid: Vec<String> = all.iter().map(|t| snake(&format!("{t:?}"))).collect();
            format!("'{field}' must be one of: {}", valid.join(", "))
        })
}

// Typed accessors: `Get` reports the current value, `Set` validates and stores.

fn b(r: &mut bool, op: Op, f: &str) -> Result<Value, String> {
    match op {
        Op::Get => Ok(json!(*r)),
        Op::Set(v) => {
            *r = boolean(v, f)?;
            Ok(Value::Null)
        }
    }
}

fn f64f(r: &mut f64, op: Op, f: &str, scale: f64, min: f64, max: f64) -> Result<Value, String> {
    match op {
        Op::Get => Ok(json!(round6(*r * scale))),
        Op::Set(v) => {
            *r = num(v, f, min, max)? / scale;
            Ok(Value::Null)
        }
    }
}

fn f32f(r: &mut f32, op: Op, f: &str, scale: f64, min: f64, max: f64) -> Result<Value, String> {
    match op {
        Op::Get => {
            let clean: f64 = r.to_string().parse().unwrap_or(*r as f64);
            Ok(json!(round6(clean * scale)))
        }
        Op::Set(v) => {
            *r = (num(v, f, min, max)? / scale) as f32;
            Ok(Value::Null)
        }
    }
}

fn u32f(r: &mut u32, op: Op, f: &str, min: u32, max: u32) -> Result<Value, String> {
    match op {
        Op::Get => Ok(json!(*r)),
        Op::Set(v) => {
            let n = num(v, f, min as f64, max as f64)?;
            if n.fract() != 0.0 {
                return Err(format!("'{f}' needs a whole number between {min} and {max}"));
            }
            *r = n as u32;
            Ok(Value::Null)
        }
    }
}

fn col(r: &mut Color, op: Op, f: &str) -> Result<Value, String> {
    match op {
        Op::Get => Ok(json!(hex_of(*r))),
        Op::Set(v) => {
            *r = colour(v, f)?;
            Ok(Value::Null)
        }
    }
}

fn opt_col(r: &mut Option<Color>, op: Op, f: &str) -> Result<Value, String> {
    match op {
        Op::Get => Ok(r.map(|c| json!(hex_of(c))).unwrap_or(Value::Null)),
        Op::Set(v) => {
            *r = if is_none_value(v) { None } else { Some(colour(v, f)?) };
            Ok(Value::Null)
        }
    }
}

fn string(r: &mut String, op: Op, f: &str, allow_empty: bool) -> Result<Value, String> {
    match op {
        Op::Get => Ok(json!(r.clone())),
        Op::Set(v) => {
            let s = v.as_str().ok_or_else(|| format!("'{f}' needs text"))?;
            if !allow_empty && s.trim().is_empty() {
                return Err(format!("'{f}' cannot be empty"));
            }
            *r = s.to_string();
            Ok(Value::Null)
        }
    }
}

fn en<T: Copy + Debug>(r: &mut T, all: &[T], op: Op, f: &str) -> Result<Value, String> {
    match op {
        Op::Get => Ok(name_of(*r)),
        Op::Set(v) => {
            *r = pick(all, v, f)?;
            Ok(Value::Null)
        }
    }
}

// -- field lists ---------------------------------------------------------------

fn hint_for(kind: &str, field: &str) -> &'static str {
    let key = bone_key(field).unwrap_or_else(|| field.to_string());
    PROPERTY_FIELDS
        .iter()
        .find(|(k, f, _, _)| *k == kind && *f == key)
        .map(|(_, _, hint, _)| *hint)
        .unwrap_or("")
}

/// `bone.3.pinned` -> `bone.<n>.pinned`.
fn bone_key(field: &str) -> Option<String> {
    let rest = field.strip_prefix("bone.")?;
    let (index, tail) = rest.split_once('.')?;
    index.parse::<usize>().ok()?;
    Some(format!("bone.<n>.{tail}"))
}

fn bone_index(field: &str) -> Option<(usize, &str)> {
    let rest = field.strip_prefix("bone.")?;
    let (index, tail) = rest.split_once('.')?;
    Some((index.parse().ok()?, tail))
}

const INSTANCE_FIELDS: &[&str] = &[
    "symbol", "first_frame", "loop_mode", "sync_to_root", "color_effect", "brightness", "alpha",
    "tint", "tint_amount",
];
/// The Set row and Include… menu of the Symbol Instance section.
const SET_FIELDS: &[&str] =
    &["symbol_set", "similar_symbols", "pick_from_set", "include_in_set", "remove_from_set"];
const TEXT_FIELDS: &[&str] =
    &["text", "text_size", "text_font", "text_bold", "text_italic", "text_align"];

fn kind_fields(kind: &str) -> impl Iterator<Item = &'static str> + '_ {
    PROPERTY_FIELDS.iter().filter(move |(k, ..)| *k == kind).map(|(_, f, ..)| *f)
}

/// The fields that apply to this very target right now.
fn valid_fields(scene: &Scene, target: &Target) -> Vec<String> {
    let all = |kind: &str| kind_fields(kind).map(String::from).collect::<Vec<_>>();
    match target {
        Target::Element(id) => {
            let Some((_, object)) = scene.find_object(*id) else {
                return Vec::new();
            };
            let mut out: Vec<String> =
                ["blend", "rotate_x", "rotate_y", "rotate_z", "z"].map(String::from).to_vec();
            if let Some(instance) = object.instance() {
                let effect = ColorEffect::from_transform(&instance.color);
                let graphic = scene
                    .library()
                    .get(instance.symbol)
                    .is_some_and(|s| s.kind.follows_parent_timeline());
                for f in INSTANCE_FIELDS {
                    let shown = match *f {
                        "loop_mode" => graphic,
                        // No effect yet: the panel's own "None" style starts a
                        // brightness or alpha at their identity, so these two
                        // can be set straight away.
                        "brightness" => matches!(effect, ColorEffect::Brightness(_) | ColorEffect::None),
                        "alpha" => matches!(effect, ColorEffect::Alpha(_) | ColorEffect::None),
                        "tint" | "tint_amount" => matches!(effect, ColorEffect::Tint { .. }),
                        _ => true,
                    };
                    if shown {
                        out.push(f.to_string());
                    }
                }
                out.extend(SET_FIELDS.iter().map(|f| f.to_string()));
            }
            if object.text.is_some() {
                out.extend(TEXT_FIELDS.iter().map(|f| f.to_string()));
            }
            if let ObjectKind::Armature(rig) = &object.kind {
                for (i, bone) in rig.armature.bones.iter().enumerate() {
                    out.push(format!("bone.{i}.name"));
                    out.push(format!("bone.{i}.pinned"));
                    out.push(format!("bone.{i}.limited"));
                    out.push(format!("bone.{i}.angle"));
                    if bone.limits.is_some() {
                        out.push(format!("bone.{i}.min_angle"));
                        out.push(format!("bone.{i}.max_angle"));
                    }
                }
            }
            out
        }
        Target::Layer(_) => all("layer"),
        Target::Frame(layer, frame) => {
            let has_sound = scene.frame_sound(*layer, *frame).is_some();
            kind_fields("frame")
                .filter(|f| !f.starts_with("sound_") || has_sound)
                .map(String::from)
                .collect()
        }
        Target::Light(id) => {
            let Some(light) = scene.lights().get(*id) else {
                return Vec::new();
            };
            kind_fields("light")
                .filter(|f| match *f {
                    "azimuth" | "elevation" => matches!(light.kind, LightKind::Sun { .. }),
                    "horizon" => matches!(light.kind, LightKind::Sky { .. }),
                    "x" | "y" | "height" | "radius" => matches!(light.kind, LightKind::Lamp { .. }),
                    "edge_x" | "edge_y" | "facing" | "throw" | "width" => {
                        matches!(light.kind, LightKind::Gloom { .. })
                    }
                    _ => true,
                })
                .map(String::from)
                .collect()
        }
        Target::LightRig => all("light_rig"),
        Target::Camera => all("camera"),
        Target::Document => all("document"),
        Target::Scene => all("scene"),
    }
}

// -- the panels' undo labels ---------------------------------------------------

fn label_of(target: &Target, field: &str) -> &'static str {
    match (target.kind(), field) {
        ("document", "frame_count") => "Document Length",
        ("document", f) if f.starts_with("loop_") => "Loop Section",
        ("document", _) => "Document Properties",
        ("scene", _) => "Rename Scene",
        ("camera", "enabled") => "Camera",
        ("camera", "locked") => "Lock Camera",
        ("camera", "focal_distance") => "Camera Depth",
        ("camera", "zoom" | "rotation" | "pitch" | "yaw") => "Camera",
        ("camera", _) => "Document Properties",
        ("layer", "depth") => "Layer Depth",
        ("layer", "blend") => "Layer Blend Mode",
        ("layer", "brightness") => "Layer Brightness",
        ("layer", "tint" | "tint_amount") => "Layer Tint",
        ("layer", _) => "Layer Properties",
        ("frame", "tween") => "Tween",
        ("frame", "ease") => "Ease Curve",
        ("frame", "label") => "Frame Label",
        ("frame", _) => "Sound Settings",
        ("element", "blend") => "Blend Mode",
        ("element", f) if f.starts_with("bone.") => match bone_index(f).map(|(_, t)| t) {
            Some("name") => "Rename Bone",
            Some("pinned") => "Pin Joint",
            // The label a bone drag's own edits carry.
            Some("angle") => "Pose",
            _ => "Joint Limits",
        },
        ("element", f) if TEXT_FIELDS.contains(&f) => "Text",
        ("element", _) => "Document Properties",
        ("light", "placement") => "Move Light",
        ("light", _) => "Light Settings",
        ("light_rig", "enabled") => "Lighting",
        ("light_rig", "base") => "Fill Light",
        ("light_rig", "modelling") => "Modelling",
        ("light_rig", "edges") => "Light Edges",
        ("light_rig", _) => "Light Model",
        _ => "Properties",
    }
}

// -- reading and writing one field -------------------------------------------------

/// One field, read (`Op::Get`) or validated and applied to `scene` (`Op::Set`).
/// Editor-level fields (`camera.enabled`, `document.frame_count`, `scene.name`,
/// text) only validate here; [`set_property`] applies them through the editor.
fn access(scene: &mut Scene, ctx: &Ctx, target: &Target, field: &str, op: Op) -> Result<Value, String> {
    match target {
        Target::Document => document(scene, field, op),
        Target::Scene => match field {
            "name" => match op {
                Op::Get => Ok(json!(ctx.scene_names.get(ctx.scene_index).cloned().unwrap_or_default())),
                Op::Set(v) => {
                    let s = v.as_str().map(str::trim).filter(|s| !s.is_empty());
                    let s = s.ok_or("'name' needs non-empty text")?;
                    let taken = ctx
                        .scene_names
                        .iter()
                        .enumerate()
                        .any(|(i, n)| i != ctx.scene_index && n == s);
                    if taken {
                        return Err(format!("a scene called '{s}' already exists"));
                    }
                    Ok(Value::Null)
                }
            },
            _ => Err(unknown(scene, target, field)),
        },
        Target::Camera => camera(scene, ctx, field, op),
        Target::Layer(id) => layer(scene, ctx, *id, field, op),
        Target::Frame(layer, frame) => frame_field(scene, ctx, *layer, *frame, field, op),
        Target::Element(id) => element(scene, ctx, *id, field, op),
        Target::Light(id) => light(scene, ctx, *id, field, op),
        Target::LightRig => {
            let mut rig = scene.lights().clone();
            let out = match field {
                "enabled" => b(&mut rig.enabled, op, field),
                "base" => col(&mut rig.base, op, field),
                "modelling" => f32f(&mut rig.modelling, op, field, 1.0, 0.0, 1.0),
                "edges" => en(&mut rig.edges, &EdgeMode::ALL, op, field),
                "model" => en(&mut rig.model, &ShadeModel::ALL, op, field),
                _ => return Err(unknown(scene, target, field)),
            }?;
            if op.is_set() {
                *scene.lights_mut() = rig;
            }
            Ok(out)
        }
    }
}

fn unknown(scene: &Scene, target: &Target, field: &str) -> String {
    format!(
        "unknown field '{field}' for {}; valid: {}",
        target.kind(),
        valid_fields(scene, target).join(", ")
    )
}

fn document(scene: &mut Scene, field: &str, op: Op) -> Result<Value, String> {
    let set = op.is_set();
    match field {
        "width" => {
            let mut w = scene.stage().size.width;
            let out = f64f(&mut w, op, field, 1.0, 1.0, 16384.0)?;
            if set {
                scene.stage_mut().size.width = w;
            }
            Ok(out)
        }
        "height" => {
            let mut h = scene.stage().size.height;
            let out = f64f(&mut h, op, field, 1.0, 1.0, 16384.0)?;
            if set {
                scene.stage_mut().size.height = h;
            }
            Ok(out)
        }
        "fps" => {
            let mut fps = scene.stage().frame_rate;
            let out = f64f(&mut fps, op, field, 1.0, 0.01, 240.0)?;
            if set {
                scene.stage_mut().frame_rate = fps;
            }
            Ok(out)
        }
        "background" => {
            let mut c = scene.stage().background;
            let out = col(&mut c, op, field)?;
            if set {
                scene.stage_mut().background = c;
            }
            Ok(out)
        }
        "sort_by_depth" => {
            let mut s = scene.stage().sort_by_depth;
            let out = b(&mut s, op, field)?;
            if set {
                scene.stage_mut().sort_by_depth = s;
            }
            Ok(out)
        }
        "frame_count" => match op {
            Op::Get => Ok(json!(scene.frame_count())),
            Op::Set(v) => {
                let n = num(v, field, 1.0, 16000.0)?;
                if n.fract() != 0.0 {
                    return Err("'frame_count' needs a whole number between 1 and 16000".into());
                }
                Ok(Value::Null)
            }
        },
        "loop_enabled" | "loop_start" | "loop_end" | "loop_repeats" => {
            let frames = scene.frame_count().max(1);
            let mut region = *scene.looping();
            let out = match field {
                "loop_enabled" => b(&mut region.enabled, op, field),
                "loop_start" => u32f(&mut region.start, op, field, 0, frames - 1),
                "loop_end" => u32f(&mut region.end, op, field, 0, frames - 1),
                _ => u32f(&mut region.repeats, op, field, 1, buzz_scene::MAX_REPEATS),
            }?;
            if set {
                *scene.looping_mut() = region.clamped(frames);
            }
            Ok(out)
        }
        _ => {
            let Some(path) = field.strip_prefix("post.") else {
                return Err(format!("unknown field '{field}' for document"));
            };
            let mut post = scene.stage().post;
            let out = match path {
                "enabled" => b(&mut post.enabled, op, field),
                "bloom.enabled" => b(&mut post.bloom.enabled, op, field),
                "bloom.threshold" => f32f(&mut post.bloom.threshold, op, field, 1.0, 0.0, 1.0),
                "bloom.intensity" => f32f(&mut post.bloom.intensity, op, field, 1.0, 0.0, 3.0),
                "bloom.radius" => f32f(&mut post.bloom.radius, op, field, 1.0, 0.0, 1.0),
                "grade.enabled" => b(&mut post.grade.enabled, op, field),
                "grade.exposure" => f32f(&mut post.grade.exposure, op, field, 1.0, -3.0, 3.0),
                "grade.contrast" => f32f(&mut post.grade.contrast, op, field, 1.0, 0.0, 2.0),
                "grade.saturation" => f32f(&mut post.grade.saturation, op, field, 1.0, 0.0, 2.0),
                "grade.temperature" => f32f(&mut post.grade.temperature, op, field, 1.0, -1.0, 1.0),
                "grade.tint" => f32f(&mut post.grade.tint, op, field, 1.0, -1.0, 1.0),
                "grade.lift" => f32f(&mut post.grade.lift, op, field, 1.0, -0.5, 0.5),
                "grade.gamma" => f32f(&mut post.grade.gamma, op, field, 1.0, 0.1, 3.0),
                "grade.gain" => f32f(&mut post.grade.gain, op, field, 1.0, 0.0, 2.0),
                "vignette.enabled" => b(&mut post.vignette.enabled, op, field),
                "vignette.amount" => f32f(&mut post.vignette.amount, op, field, 1.0, 0.0, 1.0),
                "vignette.softness" => f32f(&mut post.vignette.softness, op, field, 1.0, 0.0, 1.0),
                "vignette.color" => col(&mut post.vignette.color, op, field),
                "grain.enabled" => b(&mut post.grain.enabled, op, field),
                "grain.amount" => f32f(&mut post.grain.amount, op, field, 1.0, 0.0, 1.0),
                "grain.size" => f32f(&mut post.grain.size, op, field, 1.0, 1.0, 8.0),
                "posterise.enabled" => b(&mut post.posterise.enabled, op, field),
                "posterise.levels" => u32f(&mut post.posterise.levels, op, field, 2, 16),
                "halftone.enabled" => b(&mut post.halftone.enabled, op, field),
                "halftone.scale" => f32f(&mut post.halftone.scale, op, field, 1.0, 2.0, 24.0),
                "hatching.enabled" => b(&mut post.hatching.enabled, op, field),
                "hatching.scale" => f32f(&mut post.hatching.scale, op, field, 1.0, 2.0, 24.0),
                _ => return Err(format!("unknown field '{field}' for document")),
            }?;
            if set {
                scene.stage_mut().post = post;
            }
            Ok(out)
        }
    }
}

fn camera(scene: &mut Scene, ctx: &Ctx, field: &str, op: Op) -> Result<Value, String> {
    let set = op.is_set();
    let frame = ctx.frame;
    let cam = scene.camera();
    match field {
        "enabled" => {
            let mut e = cam.enabled;
            b(&mut e, op, field)
        }
        "locked" => {
            let mut l = cam.locked;
            let out = b(&mut l, op, field)?;
            if set {
                scene.camera_mut().locked = l;
            }
            Ok(out)
        }
        "zoom" | "rotation" | "pitch" | "yaw" => {
            let mut key = cam
                .state_at(frame)
                .map(|s| CameraKey { frame, ..s })
                .unwrap_or_else(|| CameraKey::new(frame, buzz_geom::Point::ZERO));
            let tilt = buzz_scene::MAX_TILT.to_degrees();
            let out = match field {
                "zoom" => f64f(&mut key.zoom, op, field, 1.0, 0.1, 8.0),
                "rotation" => f64f(&mut key.rotation, op, field, 180.0 / std::f64::consts::PI, -180.0, 180.0),
                "pitch" => f64f(&mut key.pitch, op, field, 180.0 / std::f64::consts::PI, -tilt, tilt),
                _ => f64f(&mut key.yaw, op, field, 180.0 / std::f64::consts::PI, -tilt, tilt),
            }?;
            if set {
                // What the Camera panel does: key the playhead and switch on.
                let camera = scene.camera_mut();
                camera.enabled = true;
                camera.set_key(key.clamped());
            }
            Ok(out)
        }
        "focal_distance" => {
            let mut d = cam.focal_distance;
            let out = f64f(&mut d, op, field, 1.0, 200.0, 6000.0)?;
            if set {
                scene.set_focal_distance(d);
            }
            Ok(out)
        }
        "aperture" | "focus_depth" => {
            let lens = cam.focus_at(frame);
            let pulling = !cam.focus_keys().is_empty();
            let (mut aperture, mut focus) = (lens.aperture, lens.focus_depth);
            let out = if field == "aperture" {
                f64f(&mut aperture, op, field, 1.0, 0.0, 0.2)
            } else {
                f64f(&mut focus, op, field, 1.0, -1.0e6, 1.0e6)
            }?;
            if set {
                buzz_ui::panels::write_lens(scene, frame, pulling, focus, aperture);
            }
            Ok(out)
        }
        "shutter" => {
            let mut s = cam.shutter;
            let out = f64f(&mut s, op, field, 1.0, 0.0, 1.0)?;
            if set {
                scene.camera_mut().shutter = s;
            }
            Ok(out)
        }
        "blur_samples" => {
            let mut n = cam.blur_samples;
            let out = u32f(&mut n, op, field, 2, buzz_scene::camera_track::MAX_BLUR_SAMPLES)?;
            if set {
                scene.camera_mut().blur_samples = n;
            }
            Ok(out)
        }
        _ => Err(format!("unknown field '{field}' for camera")),
    }
}

fn layer(scene: &mut Scene, ctx: &Ctx, id: LayerId, field: &str, op: Op) -> Result<Value, String> {
    let set = op.is_set();
    let l = scene.layers().get(id).cloned().ok_or("that layer no longer exists")?;
    let update = |scene: &mut Scene, f: &mut dyn FnMut(&mut buzz_scene::Layer)| {
        scene.update_layer(id, |layer| f(layer));
    };
    match field {
        "name" => {
            let mut v = l.name.clone();
            let out = string(&mut v, op, field, false)?;
            if set {
                update(scene, &mut |x| x.name = v.clone());
            }
            Ok(out)
        }
        "visible" => {
            let mut v = l.visible;
            let out = b(&mut v, op, field)?;
            if set {
                update(scene, &mut |x| x.visible = v);
            }
            Ok(out)
        }
        "locked" => {
            let mut v = l.locked;
            let out = b(&mut v, op, field)?;
            if set {
                update(scene, &mut |x| x.locked = v);
            }
            Ok(out)
        }
        "outline" => {
            let mut v = l.outline;
            let out = b(&mut v, op, field)?;
            if set {
                update(scene, &mut |x| x.outline = v);
            }
            Ok(out)
        }
        "unlit" => {
            let mut v = l.unlit;
            let out = b(&mut v, op, field)?;
            if set {
                update(scene, &mut |x| x.unlit = v);
            }
            Ok(out)
        }
        "casts_shadow" => {
            let mut v = l.casts_shadow;
            let out = b(&mut v, op, field)?;
            if set {
                update(scene, &mut |x| x.casts_shadow = v);
            }
            Ok(out)
        }
        "collapsed" => {
            let mut v = l.collapsed;
            let out = b(&mut v, op, field)?;
            if set {
                update(scene, &mut |x| x.collapsed = v);
            }
            Ok(out)
        }
        "alpha" => {
            let mut v = l.alpha;
            let out = f64f(&mut v, op, field, 100.0, 0.0, 100.0)?;
            if set {
                update(scene, &mut |x| x.alpha = v);
            }
            Ok(out)
        }
        "kind" => {
            const KINDS: [LayerKind; 6] = [
                LayerKind::Normal,
                LayerKind::Mask,
                LayerKind::InverseMask,
                LayerKind::Masked,
                LayerKind::Guide,
                LayerKind::Guided,
            ];
            let mut v = l.kind;
            if set && v == LayerKind::Folder {
                return Err("a folder cannot change kind: it holds layers".into());
            }
            let out = en(&mut v, &KINDS, op, field)?;
            if set {
                update(scene, &mut |x| x.kind = v);
            }
            Ok(out)
        }
        "follows" => match op {
            Op::Get => Ok(l
                .follows
                .and_then(|p| scene.layers().get(p))
                .map(|p| json!(p.name.clone()))
                .unwrap_or(Value::Null)),
            Op::Set(v) => {
                let parent = if is_none_value(v) {
                    None
                } else {
                    let text = v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
                    Some(layer_by_ref(scene, &text)?)
                };
                if let Some(p) = parent
                    && !scene.layers().can_follow(id, p)
                {
                    return Err("that layer cannot follow the chosen parent (it would loop, or it is itself)".into());
                }
                scene.set_follows(id, parent, ctx.frame);
                Ok(Value::Null)
            }
        },
        "highlight" => {
            let mut v = l.highlight;
            let out = opt_col(&mut v, op, field)?;
            if set {
                update(scene, &mut |x| x.highlight = v);
            }
            Ok(out)
        }
        "depth" => {
            let mut v = l.depth;
            let out = f64f(&mut v, op, field, 1.0, -1.0e6, 1.0e6)?;
            if set {
                update(scene, &mut |x| x.depth = v);
            }
            Ok(out)
        }
        "blend" => {
            let mut v = l.blend;
            let out = en(&mut v, &Blend::ALL, op, field)?;
            if set {
                update(scene, &mut |x| x.blend = v);
            }
            Ok(out)
        }
        "brightness" => {
            let mut v = l.brightness;
            let out = f32f(&mut v, op, field, 100.0, -100.0, 100.0)?;
            if set {
                update(scene, &mut |x| x.brightness = v.clamp(-1.0, 1.0));
            }
            Ok(out)
        }
        "tint" => {
            let mut v = l.tint;
            let out = col(&mut v, op, field)?;
            if set {
                update(scene, &mut |x| x.tint = v);
            }
            Ok(out)
        }
        "tint_amount" => {
            let mut v = l.tint_amount;
            let out = f32f(&mut v, op, field, 100.0, 0.0, 100.0)?;
            if set {
                update(scene, &mut |x| x.tint_amount = v.clamp(0.0, 1.0));
            }
            Ok(out)
        }
        _ => Err(format!("unknown field '{field}' for layer")),
    }
}

fn easing_json(e: Easing) -> Value {
    match e {
        Easing::Linear => json!("linear"),
        Easing::Strength(a) => json!(round6(a)),
        Easing::CubicBezier { x1, y1, x2, y2 } => json!([round6(x1), round6(y1), round6(x2), round6(y2)]),
    }
}

fn parse_easing(v: &Value) -> Result<Easing, String> {
    let usage = "'ease' needs \"linear\", a strength -100..100, or [x1, y1, x2, y2] with x in 0..1";
    if let Some(s) = v.as_str() {
        if s.eq_ignore_ascii_case("linear") {
            return Ok(Easing::Linear);
        }
        return as_number(v)
            .filter(|n| (-100.0..=100.0).contains(n))
            .map(Easing::Strength)
            .ok_or_else(|| usage.to_string());
    }
    if let Some(list) = v.as_array() {
        let n: Vec<f64> = list.iter().filter_map(as_number).collect();
        if n.len() == 4 && list.len() == 4 && (0.0..=1.0).contains(&n[0]) && (0.0..=1.0).contains(&n[2]) {
            return Ok(Easing::CubicBezier { x1: n[0], y1: n[1], x2: n[2], y2: n[3] });
        }
        return Err(usage.to_string());
    }
    as_number(v)
        .filter(|n| (-100.0..=100.0).contains(n))
        .map(Easing::Strength)
        .ok_or_else(|| usage.to_string())
}

fn frame_field(
    scene: &mut Scene,
    ctx: &Ctx,
    layer: LayerId,
    frame: u32,
    field: &str,
    op: Op,
) -> Result<Value, String> {
    let set = op.is_set();
    let l = scene.layers().get(layer).cloned().ok_or("that layer no longer exists")?;
    let key = l.frames.keyframe_at(frame).ok_or("no keyframe governs that frame")?.clone();
    let key_start = key.start;
    match field {
        "label" => match op {
            Op::Get => Ok(key.label.clone().map(Value::from).unwrap_or(Value::Null)),
            Op::Set(v) => {
                let label = if is_none_value(v) {
                    None
                } else {
                    Some(v.as_str().ok_or("'label' needs text, or null")?.to_string())
                };
                scene.update_layer(layer, |x| {
                    x.frames.set_label(frame, label.clone());
                });
                Ok(Value::Null)
            }
        },
        "tween" => {
            let kinds = [TweenKind::None, TweenKind::Classic, TweenKind::Motion, TweenKind::Shape];
            let mut kind = key.tween.kind;
            let out = en(&mut kind, &kinds, op, field)?;
            if set {
                if scene.layers().is_effectively_locked(layer) {
                    return Err("that layer is locked".into());
                }
                let mut tween = key.tween;
                tween.kind = kind;
                // The Tween command's own rule: an active tween needs a later keyframe.
                let has_following = l.frames.keyframes().iter().any(|k| k.start > key_start);
                if tween.is_active() && !has_following {
                    return Err("a tween needs a later keyframe to run to; insert one first".into());
                }
                scene.update_layer(layer, |x| {
                    x.frames.set_tween(key_start, tween);
                });
            }
            Ok(out)
        }
        "ease" => match op {
            Op::Get => Ok(easing_json(key.tween.easing)),
            Op::Set(v) => {
                let easing = parse_easing(v)?;
                // The Motion Editor's write: only the easing changes.
                scene.update_layer(layer, |x| {
                    let mut tween = x.frames.tween_at(frame);
                    tween.easing = easing;
                    x.frames.set_tween(frame, tween);
                });
                Ok(Value::Null)
            }
        },
        "sound" => match op {
            Op::Get => Ok(key
                .sound
                .and_then(|s| scene.sounds().get(s.sound).map(|a| json!(a.name.clone())))
                .unwrap_or(Value::Null)),
            Op::Set(v) => {
                let reference = if is_none_value(v) {
                    None
                } else {
                    let name = v.as_str().ok_or("'sound' needs a library sound name, or null")?;
                    let found = scene.sounds().iter().find(|a| a.name == name).map(|a| a.id);
                    let Some(id) = found else {
                        let names: Vec<String> = scene.sounds().iter().map(|a| a.name.clone()).collect();
                        return Err(format!("no sound '{name}' in the library; sounds: {}", names.join(", ")));
                    };
                    // Master Audio mode: only the master clip may go on a keyframe.
                    if let Some((master, master_name)) = &ctx.master_lock {
                        let is_master = match (master, scene.sounds().get(id)) {
                            (Some(m), Some(a)) => std::sync::Arc::ptr_eq(m, &a.data),
                            _ => false,
                        };
                        if !is_master {
                            return Err(crate::editor::Editor::master_audio_refusal_text(master_name));
                        }
                    }
                    // Keep the settings and swap the clip, as the panel does.
                    Some(match key.sound {
                        Some(existing) => SoundRef { sound: id, ..existing },
                        None => SoundRef::stream(id),
                    })
                };
                scene.set_frame_sound(layer, frame, reference);
                Ok(Value::Null)
            }
        },
        "sound_start" => {
            if key.sound.is_none() {
                return Err("there is no sound on that keyframe; set 'sound' first".into());
            }
            match op {
                Op::Get => Ok(json!(key_start)),
                // Validated here; `set_property` applies it through
                // `Editor::set_sound_start`, which moves the keyframe.
                Op::Set(v) => {
                    let to = num(v, field, 0.0, 16000.0)?;
                    if to.fract() != 0.0 {
                        return Err("'sound_start' needs a whole frame number".into());
                    }
                    if to as u32 == key_start {
                        return Err("the sound already starts on that frame".into());
                    }
                    Ok(Value::Null)
                }
            }
        }
        f if f.starts_with("sound_") => {
            let Some(mut r) = key.sound else {
                return Err("there is no sound on that keyframe; set 'sound' first".into());
            };
            let duration = scene.sounds().get(r.sound).map(|a| a.duration_seconds()).unwrap_or(0.0) as f32;
            let out = match f {
                "sound_sync" => en(
                    &mut r.sync,
                    &[SoundSync::Event, SoundSync::Start, SoundSync::Stop, SoundSync::Stream],
                    op,
                    f,
                ),
                "sound_volume" => f32f(&mut r.volume, op, f, 1.0, 0.0, 1.0),
                "sound_loops" => u32f(&mut r.loops, op, f, 0, 999),
                "sound_trim_start" => {
                    let upper = r.trim_end.unwrap_or(duration) as f64;
                    f32f(&mut r.trim_start, op, f, 1.0, 0.0, upper.max(0.0))
                }
                "sound_trim_end" => match op {
                    Op::Get => Ok(r.trim_end.map(|e| json!(round6(e as f64))).unwrap_or(Value::Null)),
                    Op::Set(v) => {
                        r.trim_end = if is_none_value(v) {
                            None
                        } else {
                            let end = num(v, f, r.trim_start as f64, duration as f64)? as f32;
                            (end < duration).then_some(end)
                        };
                        Ok(Value::Null)
                    }
                },
                _ => return Err(format!("unknown field '{f}' for frame")),
            }?;
            if set {
                scene.set_frame_sound(layer, frame, Some(r));
            }
            Ok(out)
        }
        _ => Err(format!("unknown field '{field}' for frame")),
    }
}

const PI_DEG: f64 = 180.0 / std::f64::consts::PI;

fn element(scene: &mut Scene, ctx: &Ctx, id: ObjectId, field: &str, op: Op) -> Result<Value, String> {
    let set = op.is_set();
    let object = scene.find_object(id).map(|(_, o)| o.clone()).ok_or("that element no longer exists")?;
    let not_here = |what: &str| format!("'{field}' needs {what}");
    match field {
        "blend" => {
            let mut v = object.blend;
            let out = en(&mut v, &Blend::ALL, op, field)?;
            if set {
                scene.update_object(id, |o| o.blend = v);
            }
            Ok(out)
        }
        "rotate_x" | "rotate_y" | "rotate_z" | "z" => {
            let mut sp = object.spatial;
            let out = match field {
                "rotate_x" => f64f(&mut sp.rotation_x, op, field, PI_DEG, -85.0, 85.0),
                "rotate_y" => f64f(&mut sp.rotation_y, op, field, PI_DEG, -85.0, 85.0),
                "rotate_z" => f64f(&mut sp.rotation_z, op, field, PI_DEG, -85.0, 85.0),
                _ => f64f(&mut sp.z, op, field, 1.0, -800.0, 800.0),
            }?;
            if set && sp != object.spatial {
                scene.update_object_where(ctx.at, id, |o| o.spatial = sp);
            }
            Ok(out)
        }
        f if INSTANCE_FIELDS.contains(&f) => {
            // The copy at the playhead, which is where `instance_field` writes
            // every field back to. Reading the earliest copy instead would put
            // its symbol back over a set pick made later on the timeline.
            let here = scene
                .find_object(id)
                .and_then(|(layer, _)| scene.layers().get(layer))
                .and_then(|layer| layer.objects_at(ctx.frame).iter().find(|o| o.id == id))
                .and_then(|o| o.instance())
                .cloned();
            let Some(instance) = here.or_else(|| object.instance().cloned()) else {
                return Err(not_here("a symbol instance"));
            };
            instance_field(scene, ctx, id, instance, f, op)
        }
        f if SET_FIELDS.contains(&f) => {
            let Some(instance) = object.instance() else {
                return Err(not_here("a symbol instance"));
            };
            set_field(scene, ctx, id, instance.symbol, f, op)
        }
        f if TEXT_FIELDS.contains(&f) => {
            let Some(text) = object.text.clone() else {
                return Err(not_here("a text object"));
            };
            let (out, _) = text_field(text, f, op)?;
            Ok(out)
        }
        f if f.starts_with("bone.") => {
            let (index, tail) = bone_index(f).ok_or_else(|| format!("unknown field '{f}'; use bone.<n>.name|pinned|limited|min_angle|max_angle|angle"))?;
            let ObjectKind::Armature(rig) = &object.kind else {
                return Err(not_here("an armature"));
            };
            let bone = rig.armature.bones.get(index).ok_or_else(|| {
                format!("this armature has {} bones; bone.{index} does not exist", rig.armature.bones.len())
            })?;
            if tail == "angle" {
                // **The pose, at the playhead.** What a bone drag writes
                // (`rigging::pose_bone` through `Document::edit("Pose")`), set
                // directly instead of by chasing a tip: the angle against the
                // parent, clamped into the joint's limits as `set_pose` does.
                // The keyframe is the playhead's own, made first when Auto
                // Keyframe is on (`update_object_where`), so a rig animated by
                // keyed angles needs no gesture. Read from the keyframe that
                // owns the playhead, not the first one the rig appears on.
                let here = scene
                    .find_object(id)
                    .and_then(|(layer, _)| scene.layers().get(layer))
                    .and_then(|layer| layer.objects_at(ctx.frame).iter().find(|o| o.id == id))
                    .and_then(|o| match &o.kind {
                        ObjectKind::Armature(rig) => rig.armature.bones.get(index).map(|b| b.angle),
                        _ => None,
                    })
                    .unwrap_or(bone.angle);
                let mut angle = here;
                let out = f64f(&mut angle, op, f, PI_DEG, -360.0, 360.0)?;
                if set {
                    scene.update_object_where(ctx.at, id, |target| {
                        if let ObjectKind::Armature(rig) = &mut target.kind {
                            let mut pose = rig.armature.pose();
                            if let Some(slot) = pose.get_mut(index) {
                                *slot = angle;
                                rig.armature.set_pose(&pose);
                            }
                        }
                    });
                }
                return Ok(out);
            }
            let mut bone = bone.clone();
            let out = match tail {
                "name" => string(&mut bone.name, op, f, false),
                "pinned" => b(&mut bone.pinned, op, f),
                "limited" => {
                    let mut limited = bone.limits.is_some();
                    let out = b(&mut limited, op, f)?;
                    if set {
                        // The panel's own starting range: a quarter turn each way.
                        bone.limits = limited.then(|| {
                            bone.limits.unwrap_or(buzz_rig::JointLimits::new(
                                -std::f64::consts::FRAC_PI_2,
                                std::f64::consts::FRAC_PI_2,
                            ))
                        });
                    }
                    Ok(out)
                }
                "min_angle" | "max_angle" => {
                    let Some(mut limits) = bone.limits else {
                        return Err("this bone has no limits; set bone.<n>.limited to true first".into());
                    };
                    let out = if tail == "min_angle" {
                        f64f(&mut limits.min, op, f, PI_DEG, -180.0, 180.0)
                    } else {
                        f64f(&mut limits.max, op, f, PI_DEG, -180.0, 180.0)
                    }?;
                    if set {
                        bone.limits = Some(buzz_rig::JointLimits::new(limits.min, limits.max));
                    }
                    Ok(out)
                }
                _ => Err(format!("unknown field '{f}'")),
            }?;
            if set {
                let new_bone = bone;
                scene.update_object(id, |target| {
                    if let ObjectKind::Armature(rig) = &mut target.kind
                        && let Some(slot) = rig.armature.bones.get_mut(index)
                    {
                        slot.name = new_bone.name.clone();
                        slot.pinned = new_bone.pinned;
                        slot.limits = new_bone.limits;
                    }
                });
            }
            Ok(out)
        }
        _ => Err(unknown(scene, &Target::Element(id), field)),
    }
}

fn instance_field(
    scene: &mut Scene,
    ctx: &Ctx,
    id: ObjectId,
    mut inst: buzz_scene::SymbolInstance,
    field: &str,
    op: Op,
) -> Result<Value, String> {
    let set = op.is_set();
    let symbol = scene.library().get(inst.symbol).ok_or("the symbol this instance refers to is no longer in the library")?;
    let (symbol_name, symbol_kind, symbol_length) = (symbol.name.clone(), symbol.kind, symbol.length());
    let effect = ColorEffect::from_transform(&inst.color);
    let need = |what: &str| format!("'{field}' applies while color_effect is {what}; set color_effect first");
    let out = match field {
        "symbol" => match op {
            Op::Get => Ok(json!(symbol_name)),
            Op::Set(v) => {
                let wanted = v.as_str().ok_or("'symbol' needs a library symbol name")?;
                let found = scene.library().iter().find(|s| s.name == wanted).map(|s| s.id);
                match found {
                    Some(sid) => {
                        inst.symbol = sid;
                        Ok(Value::Null)
                    }
                    None => {
                        let names: Vec<String> = scene.library().iter().map(|s| s.name.clone()).collect();
                        Err(format!("no symbol '{wanted}'; symbols: {}", names.join(", ")))
                    }
                }
            }
        },
        "first_frame" => {
            // Shown one-based, as Animate numbers frames.
            let mut shown = inst.first_frame + 1;
            let out = u32f(&mut shown, op, field, 1, symbol_length.max(1))?;
            inst.first_frame = shown.saturating_sub(1);
            Ok(out)
        }
        "loop_mode" => {
            if !symbol_kind.follows_parent_timeline() {
                return Err("only a graphic loops; this symbol is a movie clip or button".into());
            }
            en(&mut inst.loop_mode, &[LoopMode::Loop, LoopMode::PlayOnce, LoopMode::SingleFrame], op, field)
        }
        "sync_to_root" => b(&mut inst.sync_to_root, op, field),
        "color_effect" => match op {
            Op::Get => Ok(json!(match effect {
                ColorEffect::None => "none",
                ColorEffect::Brightness(_) => "brightness",
                ColorEffect::Tint { .. } => "tint",
                ColorEffect::Alpha(_) => "alpha",
                ColorEffect::Advanced => "advanced",
            })),
            Op::Set(v) => {
                let all = [
                    ("none", ColorEffect::None),
                    ("brightness", ColorEffect::Brightness(0.0)),
                    ("tint", ColorEffect::Tint { color: Color::from_rgba8(0xFF, 0, 0, 0xFF), amount: 0.5 }),
                    ("alpha", ColorEffect::Alpha(1.0)),
                ];
                let wanted: String = v.as_str().unwrap_or("").trim().to_ascii_lowercase();
                let Some((_, option)) = all.iter().find(|(n, _)| *n == wanted) else {
                    return Err("'color_effect' must be one of: none, brightness, tint, alpha".into());
                };
                // The panel switches only when the style actually differs.
                if std::mem::discriminant(option) != std::mem::discriminant(&effect)
                    && let Some(t) = option.to_transform()
                {
                    inst.color = t;
                }
                Ok(Value::Null)
            }
        },
        "brightness" => {
            let amount = match effect {
                ColorEffect::Brightness(a) => a,
                ColorEffect::None => 0.0,
                _ => return Err(need("brightness or none")),
            };
            let mut percent = amount;
            let out = f32f(&mut percent, op, field, 100.0, -100.0, 100.0)?;
            inst.color = ColorTransform::brightness(percent);
            Ok(out)
        }
        "alpha" => {
            let amount = match effect {
                ColorEffect::Alpha(a) => a,
                ColorEffect::None => 1.0,
                _ => return Err(need("alpha or none")),
            };
            let mut fraction = amount;
            let out = f32f(&mut fraction, op, field, 100.0, 0.0, 100.0)?;
            inst.color = ColorTransform::alpha(fraction);
            Ok(out)
        }
        "tint" | "tint_amount" => {
            let ColorEffect::Tint { color, amount } = effect else { return Err(need("tint")) };
            let (mut c, mut a) = (color, amount);
            let out = if field == "tint" {
                col(&mut c, op, field)
            } else {
                f32f(&mut a, op, field, 100.0, 0.0, 100.0)
            }?;
            inst.color = ColorTransform::tint(c, a);
            Ok(out)
        }
        _ => Err(format!("unknown field '{field}'")),
    }?;
    if set {
        let new_symbol = inst.symbol;
        let (first, mode, color, sync) = (inst.first_frame, inst.loop_mode, inst.color, inst.sync_to_root);
        scene.update_object_where(ctx.at, id, |o| {
            if let ObjectKind::Instance(i) = &mut o.kind {
                i.symbol = new_symbol;
                i.first_frame = first;
                i.loop_mode = mode;
                i.color = color;
                i.sync_to_root = sync;
            }
        });
        // A swap can leave the instance past the end of its new symbol; pull it back.
        let length = scene.library().get(new_symbol).map(|s| s.length()).unwrap_or(1);
        if first >= length {
            scene.update_object_where(ctx.at, id, |o| {
                if let ObjectKind::Instance(i) = &mut o.kind {
                    i.first_frame = length.saturating_sub(1);
                }
            });
        }
    }
    Ok(out)
}

/// The Set row and Include… menu: the same lists the panel offers and the
/// same calls it makes (`Library::include_in_set` / `remove_from_set`,
/// `Scene::pick_from_set` at the playhead).
fn set_field(
    scene: &mut Scene,
    ctx: &Ctx,
    id: ObjectId,
    fallback: buzz_scene::SymbolId,
    field: &str,
    op: Op,
) -> Result<Value, String> {
    // The symbol shown at the playhead: a pick keys the layer there, so it can
    // differ from the earliest copy of the instance.
    let shown = scene
        .find_object(id)
        .and_then(|(layer, _)| scene.layers().get(layer))
        .and_then(|layer| layer.objects_at(ctx.frame).iter().find(|o| o.id == id))
        .and_then(|o| o.instance())
        .map(|i| i.symbol)
        .unwrap_or(fallback);
    let library = scene.library();
    let name_of = |sid: buzz_scene::SymbolId| library.get(sid).map(|s| s.name.clone()).unwrap_or_default();
    let set: Vec<buzz_scene::SymbolId> = library.symbol_set(shown);
    let owner = set.first().copied().unwrap_or(shown);
    let in_some_set: std::collections::HashSet<buzz_scene::SymbolId> =
        library.iter().flat_map(|s| s.set.iter().copied()).collect();
    let includable: Vec<buzz_scene::SymbolId> = library
        .iter()
        .filter(|s| s.id != owner && s.set.is_empty() && !in_some_set.contains(&s.id))
        .map(|s| s.id)
        .collect();
    let removable: Vec<buzz_scene::SymbolId> =
        set.iter().skip(1).copied().filter(|sid| *sid != shown).collect();
    let names = |list: &[buzz_scene::SymbolId]| list.iter().map(|sid| name_of(*sid)).collect::<Vec<_>>();
    let by_name = |wanted: &str, among: &[buzz_scene::SymbolId], what: &str| {
        among.iter().copied().find(|sid| name_of(*sid) == wanted).ok_or_else(|| {
            format!("'{wanted}' {what}; valid: {}", names(among).join(", "))
        })
    };

    match (field, op) {
        ("symbol_set", Op::Get) => Ok(json!(names(&set))),
        ("symbol_set", Op::Set(_)) => {
            Err("'symbol_set' is read-only; use include_in_set, remove_from_set or pick_from_set".into())
        }
        ("similar_symbols", Op::Get) => {
            let bounds = scene.cached_bounds_table();
            let similar: Vec<buzz_scene::SymbolId> =
                library.similar_symbols(shown, &bounds).into_iter().map(|(sid, _)| sid).collect();
            Ok(json!(names(&similar)))
        }
        ("similar_symbols", Op::Set(_)) => {
            Err("'similar_symbols' is read-only; set 'symbol' to swap".into())
        }
        ("include_in_set", Op::Get) => Ok(json!(names(&includable))),
        ("remove_from_set", Op::Get) => Ok(json!(names(&removable))),
        ("pick_from_set", Op::Get) => Ok(json!(name_of(shown))),
        ("include_in_set", Op::Set(v)) => {
            let wanted = v.as_str().ok_or("'include_in_set' needs a library symbol name")?;
            let member = by_name(wanted, &includable, "cannot be included in this set")?;
            scene.library_mut().include_in_set(owner, member);
            Ok(Value::Null)
        }
        ("remove_from_set", Op::Set(v)) => {
            let wanted = v.as_str().ok_or("'remove_from_set' needs a set member name")?;
            let member = by_name(wanted, &removable, "cannot be removed from this set")?;
            scene.library_mut().remove_from_set(owner, member);
            Ok(Value::Null)
        }
        ("pick_from_set", Op::Set(v)) => {
            let (wanted, start) = match v {
                Value::String(name) => (name.as_str(), "same_frame"),
                Value::Object(map) => (
                    map.get("symbol").and_then(Value::as_str).ok_or("'pick_from_set' needs {\"symbol\": name}")?,
                    map.get("start").and_then(Value::as_str).unwrap_or("same_frame"),
                ),
                _ => return Err("'pick_from_set' needs a set member name".into()),
            };
            let start = match start {
                "same_frame" => buzz_scene::SetStart::SameFrame,
                "from_start" => buzz_scene::SetStart::FromStart,
                other => return Err(format!("'start' must be one of: same_frame, from_start (got '{other}')")),
            };
            if set.is_empty() {
                return Err("this instance's symbol is in no set; include_in_set first".into());
            }
            let others: Vec<buzz_scene::SymbolId> = set.iter().copied().filter(|sid| *sid != shown).collect();
            let to = by_name(wanted, &others, "is not another member of this instance's set")?;
            if !scene.pick_from_set(ctx.frame, id, to, start) {
                return Err(format!("could not pick '{wanted}' here"));
            }
            Ok(Value::Null)
        }
        _ => Err(format!("unknown field '{field}'")),
    }
}

/// Read, or compute the new `TextData` for, one text field.
fn text_field(mut text: buzz_scene::TextData, field: &str, op: Op) -> Result<(Value, buzz_scene::TextData), String> {
    let out = match field {
        "text" => string(&mut text.content, op, field, true),
        "text_size" => f64f(&mut text.size, op, field, 1.0, 4.0, 400.0),
        "text_font" => match op {
            Op::Get => Ok(text.font.clone().map(Value::from).unwrap_or(Value::Null)),
            Op::Set(v) => {
                text.font = if is_none_value(v) || v.as_str() == Some("Default") {
                    None
                } else {
                    Some(v.as_str().ok_or("'text_font' needs a family name, or null")?.to_string())
                };
                Ok(Value::Null)
            }
        },
        "text_bold" => b(&mut text.style.bold, op, field),
        "text_italic" => b(&mut text.style.italic, op, field),
        "text_align" => en(&mut text.align, &TextAlign::ALL, op, field),
        _ => Err(format!("unknown field '{field}'")),
    }?;
    Ok((out, text))
}

fn light(scene: &mut Scene, ctx: &Ctx, id: LightId, field: &str, op: Op) -> Result<Value, String> {
    let set = op.is_set();
    if field == "placement" {
        return light_placement(scene, id, op);
    }
    // The light as it looks at the playhead, which is what the panel edits.
    let mut l = scene
        .lights()
        .keyed_at(ctx.frame)
        .get(id)
        .cloned()
        .ok_or("that light no longer exists")?;
    let wrong = |what: &str| format!("'{field}' belongs to a {what} light");
    let out = match field {
        "name" => string(&mut l.name, op, field, false),
        "enabled" => b(&mut l.enabled, op, field),
        "color" => col(&mut l.color, op, field),
        "intensity" => {
            let max = if l.is_gloom() { 1.0 } else { 4.0 };
            f32f(&mut l.intensity, op, field, 1.0, 0.0, max)
        }
        "softness" => f64f(&mut l.softness, op, field, 1.0, 0.0, 1.0),
        "glow" => f32f(&mut l.glow, op, field, 1.0, 0.0, 1.0),
        "flicker" => f32f(&mut l.flicker, op, field, 1.0, 0.0, 1.0),
        "rim" => f32f(&mut l.rim, op, field, 1.0, 0.0, 1.0),
        "glint" => f32f(&mut l.glint, op, field, 1.0, 0.0, 1.0),
        "storm" => f32f(&mut l.storm, op, field, 1.0, 0.0, 1.0),
        "shadows" => b(&mut l.shadows, op, field),
        "fall" => en(&mut l.fall, &ShadowFall::ALL, op, field),
        "shadow_strength" => f32f(&mut l.shadow_strength, op, field, 1.0, 0.0, 1.0),
        "shadow_length" => {
            let (lo, hi) = (*buzz_scene::SHADOW_LENGTH_RANGE.start() as f64, *buzz_scene::SHADOW_LENGTH_RANGE.end() as f64);
            f32f(&mut l.shadow_length, op, field, 1.0, lo, hi)
        }
        "standing_height" => f64f(&mut l.standing_height, op, field, 1.0, 0.0, 400.0),
        "shadow_offset_x" => f64f(&mut l.shadow_adjustment.offset.x, op, field, 1.0, -1.0e6, 1.0e6),
        "shadow_offset_y" => f64f(&mut l.shadow_adjustment.offset.y, op, field, 1.0, -1.0e6, 1.0e6),
        "shadow_scale_x" => f64f(&mut l.shadow_adjustment.scale_x, op, field, 1.0, 0.01, 100.0),
        "shadow_scale_y" => f64f(&mut l.shadow_adjustment.scale_y, op, field, 1.0, 0.01, 100.0),
        "shadow_rotation" => f64f(&mut l.shadow_adjustment.rotation, op, field, PI_DEG, -3600.0, 3600.0),
        "shadow_skew_x" => f64f(&mut l.shadow_adjustment.skew_x, op, field, PI_DEG, -80.0, 80.0),
        "shadow_skew_y" => f64f(&mut l.shadow_adjustment.skew_y, op, field, PI_DEG, -80.0, 80.0),
        "shadow_pivot_x" => f64f(&mut l.shadow_adjustment.pivot.x, op, field, 1.0, -1.0e6, 1.0e6),
        "shadow_pivot_y" => f64f(&mut l.shadow_adjustment.pivot.y, op, field, 1.0, -1.0e6, 1.0e6),
        "azimuth" | "elevation" => match &mut l.kind {
            LightKind::Sun { azimuth, elevation } => {
                if field == "azimuth" {
                    f64f(azimuth, op, field, PI_DEG, -180.0, 180.0)
                } else {
                    f64f(elevation, op, field, PI_DEG, 0.0, 90.0)
                }
            }
            _ => Err(wrong("sun")),
        },
        "horizon" => match &mut l.kind {
            LightKind::Sky { horizon } => col(horizon, op, field),
            _ => Err(wrong("sky")),
        },
        "x" | "y" | "height" | "radius" => match &mut l.kind {
            LightKind::Lamp { position, height, radius } => match field {
                "x" => f64f(&mut position.x, op, field, 1.0, -1.0e6, 1.0e6),
                "y" => f64f(&mut position.y, op, field, 1.0, -1.0e6, 1.0e6),
                "height" => f64f(height, op, field, 1.0, 4.0, 1200.0),
                _ => f64f(radius, op, field, 1.0, 40.0, 3000.0),
            },
            _ => Err(wrong("lamp")),
        },
        "edge_x" | "edge_y" | "facing" | "throw" | "width" => match &mut l.kind {
            LightKind::Gloom { edge, facing, throw, width } => match field {
                "edge_x" => f64f(&mut edge.x, op, field, 1.0, -1.0e6, 1.0e6),
                "edge_y" => f64f(&mut edge.y, op, field, 1.0, -1.0e6, 1.0e6),
                "facing" => f64f(facing, op, field, PI_DEG, -180.0, 180.0),
                "throw" => f64f(throw, op, field, 1.0, 100.0, 6000.0),
                _ => f64f(width, op, field, 1.0, 100.0, 8000.0),
            },
            _ => Err(wrong("gloom")),
        },
        _ => Err(format!("unknown field '{field}' for light")),
    }?;
    if set {
        crate::lights::apply_light_edit(scene, l, ctx.frame, ctx.auto_key);
    }
    Ok(out)
}

/// Where a light sits in the layer stack: the index of the stage layer it is
/// directly above, or null for the top. Applied through `Editor::place_light`.
fn light_placement(scene: &mut Scene, id: LightId, op: Op) -> Result<Value, String> {
    let stack: Vec<LayerId> = scene.stage_layers().iter().map(|l| l.id).collect();
    match op {
        Op::Get => Ok(scene
            .light_slot(id)
            .and_then(|l| scene.stage_layer_index(l))
            .map(|i| json!(i))
            .unwrap_or(Value::Null)),
        Op::Set(v) => {
            let above = placement_above(v, &stack)?;
            if !scene.place_light(id, above) {
                return Err("the light already sits there".into());
            }
            Ok(Value::Null)
        }
    }
}

fn placement_above(v: &Value, stack: &[LayerId]) -> Result<Option<LayerId>, String> {
    if is_none_value(v) {
        return Ok(None);
    }
    let n = as_number(v)
        .filter(|n| n.fract() == 0.0 && *n >= 0.0)
        .ok_or("'placement' needs a stage-layer index, or null for the top of the stack")?;
    stack.get(n as usize).copied().map(Some).ok_or_else(|| {
        format!("'placement' must be between 0 and {} (stage layers), or null", stack.len().saturating_sub(1))
    })
}

// -- the tools ---------------------------------------------------------------------

fn context(app: &App) -> Ctx {
    let editor = app.agent_editor();
    Ctx {
        frame: editor.current_frame,
        at: editor.edit_at(),
        auto_key: editor.auto_keyframe,
        scene_index: editor.doc.active_scene(),
        scene_names: editor.doc.scene_names(),
        master_lock: editor.master_audio_lock(),
    }
}

/// `set_property {target, field, value}`.
pub fn set_property(app: &mut App, args: &Value) -> AgentResult {
    let Some(target_text) = args.get("target").and_then(|v| v.as_str()) else {
        return AgentResult::err(format!("set_property needs a 'target': {TARGET_FORMS}"));
    };
    let Some(field) = args.get("field").and_then(|v| v.as_str()) else {
        return AgentResult::err("set_property needs a 'field'; inspect {what:\"properties\", target} lists them");
    };
    let Some(value) = args.get("value") else {
        return AgentResult::err("set_property needs a 'value'");
    };
    let field = field.trim();

    // **The playhead**, which a person moves by clicking a frame. It is view
    // state rather than a document edit — no undo step, as with that click —
    // so it is answered here instead of through the scene's fields. Without
    // it, "key the lamp at frame 24" meant 23 `next_frame` calls, and a live
    // model ran out of turns before it got there.
    if target_text.trim() == "document" && field == "playhead" {
        let count = app.agent_editor().doc.scene().frame_count().max(1);
        let Some(frame) = value.as_u64().or_else(|| value.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
        else {
            return AgentResult::err(format!("'playhead' needs a 0-based frame number 0..{}", count - 1));
        };
        if frame >= u64::from(count) {
            return AgentResult::err(format!("frame {frame} is past the end; frames are 0..{}", count - 1));
        }
        app.agent_editor_mut().set_frame(frame as u32);
        return AgentResult::ok(json!({
            "target": "document",
            "field": "playhead",
            "value": app.agent_editor().current_frame,
        }));
    }

    let ctx = context(app);
    let scene = app.agent_editor().doc.scene().clone();
    let target = match parse_target(&scene, target_text) {
        Ok(t) => t,
        Err(e) => return AgentResult::err(e),
    };
    let valid = valid_fields(&scene, &target);
    if !valid.iter().any(|f| f == field) {
        // A field that exists but needs another setting first says so itself.
        let mut probe = scene.clone();
        let known = PROPERTY_FIELDS.iter().any(|(k, f, ..)| {
            *k == target.kind() && (*f == field || bone_key(field).as_deref() == Some(*f))
        });
        if known {
            if let Err(e) = access(&mut probe, &ctx, &target, field, Op::Set(value)) {
                return AgentResult::err(format!("{e}; valid: {}", valid.join(", ")));
            }
        }
        return AgentResult::err(unknown(&scene, &target, field));
    }

    // Validate on a copy: nothing changes unless this passes.
    let mut trial = scene;
    if let Err(e) = access(&mut trial, &ctx, &target, field, Op::Set(value)) {
        return AgentResult::err(e);
    }

    let label = label_of(&target, field);
    let depth_before = app.agent_editor().doc.history().undo_depth();
    let editor = app.agent_editor_mut();
    editor.doc.end_gesture();
    match (&target, field) {
        (Target::Camera, "enabled") => {
            let want = boolean(value, field).unwrap_or(false);
            if editor.doc.scene().camera().enabled != want {
                editor.run(buzz_ui::Command::ToggleCamera);
            }
        }
        (Target::Document, "frame_count") => {
            let n = num(value, field, 1.0, 16000.0).unwrap_or(1.0) as u32;
            editor.set_frame_count(n);
        }
        (Target::Scene, "name") => {
            let name = value.as_str().unwrap_or("").trim().to_string();
            let index = editor.doc.active_scene();
            editor.doc.rename_scene(index, name);
        }
        (Target::Frame(layer, frame), "sound_start") => {
            let to = num(value, field, 0.0, 16000.0).unwrap_or(0.0) as u32;
            let from = editor
                .doc
                .scene()
                .layers()
                .get(*layer)
                .and_then(|l| l.frames.keyframe_at(*frame))
                .map(|k| k.start);
            if let Some(from) = from {
                editor.set_sound_start(*layer, from, to);
            }
            let scene = editor.doc.scene().clone();
            editor.sound.refresh(&scene);
        }
        (Target::Light(id), "placement") => {
            let stack: Vec<LayerId> = editor.doc.scene().stage_layers().iter().map(|l| l.id).collect();
            let above = placement_above(value, &stack).unwrap_or(None);
            editor.place_light(*id, above);
        }
        (Target::Element(id), f) if TEXT_FIELDS.contains(&f) => {
            let id = *id;
            if let Some(current) = editor.text_of(id)
                && let Ok((_, new)) = text_field(current, f, Op::Set(value))
            {
                editor.set_text_styled(id, new.content, new.size, new.font, new.style, new.align);
            }
        }
        _ => {
            editor.doc.edit(label, |scene| {
                let _ = access(scene, &ctx, &target, field, Op::Set(value));
            });
            if matches!(target, Target::Frame(..)) && field.starts_with("sound") {
                let scene = editor.doc.scene().clone();
                editor.sound.refresh(&scene);
            }
        }
    }
    editor.doc.end_gesture();

    let mut now = editor.doc.scene().clone();
    let current = access(&mut now, &context(app), &target, field, Op::Get).unwrap_or(Value::Null);
    let depth_after = app.agent_editor().doc.history().undo_depth();
    AgentResult::ok(json!({
        "target": target_text,
        "field": field,
        "value": current,
        "undo_label": label,
        "undo_steps_added": depth_after as i64 - depth_before as i64,
        "undo_depth": depth_after,
    }))
}

/// `inspect {what: "properties", target}`: every field with its current value.
pub fn inspect_properties(app: &App, args: &Value) -> AgentResult {
    let Some(target_text) = args.get("target").and_then(|v| v.as_str()) else {
        return AgentResult::err(format!("inspect properties needs a 'target': {TARGET_FORMS}"));
    };
    let ctx = context(app);
    let mut scene = app.agent_editor().doc.scene().clone();
    let target = match parse_target(&scene, target_text) {
        Ok(t) => t,
        Err(e) => return AgentResult::err(e),
    };
    let mut fields = Vec::new();
    for field in valid_fields(&scene, &target) {
        let value = access(&mut scene, &ctx, &target, &field, Op::Get).unwrap_or(Value::Null);
        fields.push(json!({
            "field": field,
            "value": value,
            "hint": hint_for(target.kind(), &field),
        }));
    }
    AgentResult::ok(json!({"target": target_text, "kind": target.kind(), "fields": fields}))
}
