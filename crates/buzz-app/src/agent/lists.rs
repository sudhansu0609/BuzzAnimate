//! D2 of `docs/AI_AGENT_PLAN.md`: `edit_list {target, list, op, index?, to?, item?}`.
//!
//! The list-structured edits `set_property` cannot express, made through the
//! panels' own mutation code: the Filters panel (`Editor::edit_filter_list`),
//! Live Motion (`add_modifier` / `remove_modifier` / `set_modifier`), the
//! Swatches panel (the same `Scene` palette calls it makes, inside its
//! `"Swatches"` edit) and the Rigging panel (`Editor::rig_*`,
//! `apply_named_pose`), all in `crate::list_edit` where the panels call them
//! too. Every op is validated before anything is touched, is one undo step
//! (`undo_steps_added` says so; applying a pose with `key:true` is two, the
//! panel's own Key Pose being a keyframe and then a pose), and returns the
//! list as it now stands. Swatches live in the scene, so they are in the
//! history like everything else.
//!
//! | list | target | ops |
//! |---|---|---|
//! | `filters` | `element:<id>` or `layer:<ref>` | get add remove move set |
//! | `modifiers` | `element:<id>` | get add remove set |
//! | `swatches` | `document` | get add remove set move |
//! | `swatch_folders` | `document` | get add remove |
//! | `poses` | `element:<id>` of an armature | get add set remove |
//! | `bones` | `element:<id>` of an armature | get remove move |
//! | `rig` | `element:<id>` of an armature | get set_rest_pose mirror_pose reset_pose |

use super::props::{Target, parse_target};
use super::{AgentResult, hex_of, norm, parse_hex};
use crate::app::App;
use buzz_scene::{Filter, FilterKind, LayerId, Modifier, ObjectId, ObjectKind, Scene, SwatchId};
use serde_json::{Map, Value, json};

const LISTS: &[(&str, &[&str])] = &[
    ("filters", &["get", "add", "remove", "move", "set"]),
    ("modifiers", &["get", "add", "remove", "set"]),
    ("swatches", &["get", "add", "remove", "set", "move"]),
    ("swatch_folders", &["get", "add", "remove"]),
    ("poses", &["get", "add", "set", "remove"]),
    ("bones", &["get", "remove", "move"]),
    ("rig", &["get", "set_rest_pose", "mirror_pose", "reset_pose"]),
];

/// What one op needs besides the app.
struct Req<'a> {
    op: &'a str,
    index: Option<usize>,
    to: Option<&'a Value>,
    item: Option<&'a Value>,
}

pub fn edit_list(app: &mut App, args: &Value) -> AgentResult {
    if let Some(map) = args.as_object() {
        for key in map.keys() {
            if !["target", "list", "op", "index", "to", "item"].contains(&key.as_str()) {
                return AgentResult::err(format!("unknown arg '{key}'; valid: target, list, op, index, to, item"));
            }
        }
    }
    let s = |key: &str| args.get(key).and_then(|v| v.as_str());
    let (Some(target_text), Some(list), Some(op)) = (s("target"), s("list"), s("op")) else {
        return AgentResult::err("edit_list needs 'target', 'list' and 'op'; see the catalog entry for the combinations");
    };
    let Some((_, ops)) = LISTS.iter().find(|(l, _)| *l == list) else {
        let names: Vec<&str> = LISTS.iter().map(|(l, _)| *l).collect();
        return AgentResult::err(format!("unknown list '{list}'; valid: {}", names.join(", ")));
    };
    if !ops.contains(&op) {
        return AgentResult::err(format!("list '{list}' has no op '{op}'; valid: {}", ops.join(", ")));
    }
    let index = match args.get("index").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => match v.as_f64().filter(|n| *n >= 0.0 && n.fract() == 0.0) {
            Some(n) => Some(n as usize),
            None => return AgentResult::err("'index' needs a non-negative whole number"),
        },
    };
    let req = Req {
        op,
        index,
        to: args.get("to"),
        item: args.get("item").filter(|v| !v.is_null()),
    };

    let scene = app.agent_editor().doc.scene().clone();
    let target = match parse_target(&scene, target_text) {
        Ok(t) => t,
        Err(e) => return AgentResult::err(e),
    };

    let before = app.agent_editor().doc.history().undo_depth();
    if op != "get" {
        app.agent_editor_mut().doc.end_gesture();
    }
    let extra = match list {
        "filters" => filters_apply(app, &target, &req),
        "modifiers" => modifiers_apply(app, &target, &req),
        "swatches" => swatches_apply(app, &target, &req),
        "swatch_folders" => folders_apply(app, &target, &req),
        "poses" => poses_apply(app, &target, &req),
        "bones" => bones_apply(app, &target, &req),
        _ => rig_apply(app, &target, &req),
    };
    let extra = match extra {
        Ok(e) => e,
        Err(e) => return AgentResult::err(e),
    };
    if op != "get" {
        app.agent_editor_mut().doc.end_gesture();
    }

    let now = app.agent_editor().doc.scene().clone();
    let items = match list {
        "filters" => filters_items(&now, &target),
        "modifiers" => modifiers_items(&now, &target),
        "swatches" => Ok(swatch_items(&now)),
        "swatch_folders" => Ok(folder_items(&now)),
        "poses" => poses_items(&now, &target),
        "bones" => bones_items(&now, &target),
        _ => bones_items(&now, &target),
    }
    .unwrap_or(Value::Null);
    let editor = app.agent_editor();
    let after = editor.doc.history().undo_depth();
    let mut value = json!({
        "target": target_text,
        "list": list,
        "op": op,
        "items": items,
        "undo_steps_added": after as i64 - before as i64,
        "undo_depth": after,
        "can_undo": editor.doc.can_undo(),
        "status": editor.status,
    });
    if let (Some(map), Some(more)) = (value.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            map.insert(k.clone(), v.clone());
        }
    }
    AgentResult::ok(value)
}

// -- shared helpers ----------------------------------------------------------------

fn index_of(req: &Req, len: usize, what: &str) -> Result<usize, String> {
    let i = req
        .index
        .ok_or_else(|| format!("op {} needs 'index' (0..{})", req.op, len.saturating_sub(1)))?;
    if i >= len {
        return Err(if len == 0 {
            format!("the {what} list is empty")
        } else {
            format!("'index' {i} is out of range; the {what} list has {len} item(s), 0..{}", len - 1)
        });
    }
    Ok(i)
}

fn item_object<'a>(req: &Req<'a>) -> Result<&'a Map<String, Value>, String> {
    req.item
        .and_then(|v| v.as_object())
        .ok_or_else(|| format!("op {} needs 'item' as an object", req.op))
}

fn unknown_keys(item: &Map<String, Value>, valid: &[&str]) -> Result<(), String> {
    for key in item.keys() {
        if !valid.contains(&key.as_str()) {
            return Err(format!("unknown item field '{key}'; valid: {}", valid.join(", ")));
        }
    }
    Ok(())
}

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

/// Colours in serde's form (`{"components": [r,g,b,a]}`) become `#rrggbb[aa]`.
fn hexify(v: Value) -> Value {
    match v {
        Value::Object(map) if map.len() == 1 && map.get("components").is_some_and(|c| c.is_array()) => {
            match serde_json::from_value::<peniko::Color>(Value::Object(map.clone())) {
                Ok(c) => json!(hex_of(c)),
                Err(_) => Value::Object(map),
            }
        }
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, hexify(v))).collect()),
        Value::Array(list) => Value::Array(list.into_iter().map(hexify).collect()),
        other => other,
    }
}

fn is_colour(v: &Value) -> bool {
    v.as_object().is_some_and(|m| m.len() == 1 && m.get("components").is_some_and(|c| c.is_array()))
}

fn armature_of(scene: &Scene, target: &Target, list: &str) -> Result<(ObjectId, buzz_rig::Armature), String> {
    let Target::Element(id) = target else {
        return Err(format!("list '{list}' lives on element:<id> of an armature"));
    };
    match scene.find_object(*id).map(|(_, o)| &o.kind) {
        Some(ObjectKind::Armature(rig)) => Ok((*id, rig.armature.clone())),
        _ => Err(format!("element {} is not an armature (a rigged character); list '{list}' needs one", id.0)),
    }
}

// -- filters --------------------------------------------------------------------------

fn filters_of(scene: &Scene, target: &Target) -> Result<Vec<Filter>, String> {
    match target {
        Target::Element(id) => scene
            .find_object(*id)
            .map(|(_, o)| o.filters.clone())
            .ok_or_else(|| format!("no element {}", id.0)),
        Target::Layer(l) => scene
            .layers()
            .get(*l)
            .map(|l| l.filters.clone())
            .ok_or_else(|| "that layer no longer exists".to_string()),
        _ => Err("list 'filters' lives on element:<id> or layer:<index|name>".into()),
    }
}

fn filters_items(scene: &Scene, target: &Target) -> Result<Value, String> {
    let list = filters_of(scene, target)?;
    Ok(Value::Array(
        list.iter()
            .enumerate()
            .map(|(index, f)| {
                let v = serde_json::to_value(f).unwrap_or(Value::Null);
                let (kind, params) = v["kind"]
                    .as_object()
                    .and_then(|m| m.iter().next())
                    .map(|(k, p)| (snake(k), hexify(p.clone())))
                    .unwrap_or_default();
                json!({"index": index, "kind": kind, "enabled": f.enabled, "params": params})
            })
            .collect(),
    ))
}

/// A filter from `item`: `{kind, enabled?, <parameters>}`, on top of `base` (the
/// existing filter when setting) or the kind's own defaults (when adding).
fn build_filter(item: &Map<String, Value>, base: Option<&Filter>) -> Result<Filter, String> {
    let defaults = FilterKind::all();
    let variant_of = |k: &FilterKind| -> String {
        serde_json::to_value(k)
            .ok()
            .and_then(|v| v.as_object().and_then(|m| m.keys().next().cloned()))
            .unwrap_or_default()
    };
    let wanted = item.get("kind").and_then(|v| v.as_str());
    let start: Filter = match (base, wanted) {
        (Some(b), None) => b.clone(),
        (Some(b), Some(k)) => {
            if norm(&variant_of(&b.kind)) != norm(k) && norm(b.kind.label()) != norm(k) {
                return Err(format!(
                    "a {} filter cannot become '{k}'; remove it and add the other",
                    b.kind.label()
                ));
            }
            b.clone()
        }
        (None, Some(k)) => {
            let found = defaults
                .iter()
                .find(|d| norm(&variant_of(d)) == norm(k) || norm(d.label()) == norm(k));
            match found {
                Some(d) => Filter::new(d.clone()),
                None => {
                    let names: Vec<String> = defaults.iter().map(|d| snake(&variant_of(d))).collect();
                    return Err(format!("unknown filter kind '{k}'; valid: {}", names.join(", ")));
                }
            }
        }
        (None, None) => {
            let names: Vec<String> = defaults.iter().map(|d| snake(&variant_of(d))).collect();
            return Err(format!("item needs a 'kind'; valid: {}", names.join(", ")));
        }
    };

    let mut v = serde_json::to_value(&start).map_err(|e| e.to_string())?;
    let variant = variant_of(&start.kind);
    let mut valid: Vec<String> = vec!["kind".into(), "enabled".into()];
    for (key, val) in item {
        match key.as_str() {
            "kind" => {}
            "enabled" => {
                v["enabled"] = json!(val.as_bool().ok_or("'enabled' needs true or false")?);
            }
            _ => {
                let params = v["kind"][&variant]
                    .as_object_mut()
                    .ok_or("this filter has no parameters")?;
                if !params.contains_key(key) {
                    valid.extend(params.keys().cloned());
                    return Err(format!(
                        "unknown parameter '{key}' for {}; valid: {}",
                        snake(&variant),
                        valid.join(", ")
                    ));
                }
                let existing = params[key].clone();
                let new = if is_colour(&existing) {
                    let c = val
                        .as_str()
                        .and_then(parse_hex)
                        .ok_or_else(|| format!("'{key}' needs a colour like \"#rrggbb\""))?;
                    serde_json::to_value(c).map_err(|e| e.to_string())?
                } else if existing.is_string() {
                    // An enum such as quality or bevel type, named in any case.
                    let text = val.as_str().ok_or_else(|| format!("'{key}' needs a name"))?;
                    let mut cs = text.chars();
                    let cap = cs
                        .next()
                        .map(|f| f.to_ascii_uppercase().to_string() + &cs.as_str().to_ascii_lowercase())
                        .unwrap_or_default();
                    json!(cap)
                } else {
                    val.clone()
                };
                params.insert(key.clone(), new);
            }
        }
    }
    serde_json::from_value::<Filter>(v).map_err(|e| format!("a filter parameter has a bad value: {e}"))
}

fn filters_apply(app: &mut App, target: &Target, req: &Req) -> Result<Value, String> {
    use buzz_ui::FilterTarget;
    let scene = app.agent_editor().doc.scene().clone();
    let current = filters_of(&scene, target)?;
    let (ft, object, layer): (FilterTarget, Option<ObjectId>, Option<LayerId>) = match target {
        Target::Element(id) => (FilterTarget::Object, Some(*id), None),
        Target::Layer(l) => (FilterTarget::Layer, None, Some(*l)),
        _ => unreachable!("filters_of refused it"),
    };
    let editor_edit = |app: &mut App, label: &'static str, change: &dyn Fn(&mut Vec<Filter>)| {
        app.agent_editor_mut().edit_filter_list(ft, object, layer, label, change);
    };
    match req.op {
        "get" => {}
        "add" => {
            let f = build_filter(item_object(req)?, None)?;
            editor_edit(app, "Add Filter", &move |l| l.push(f.clone()));
        }
        "remove" => {
            let i = index_of(req, current.len(), "filters")?;
            editor_edit(app, "Delete Filter", &move |l| {
                if i < l.len() {
                    l.remove(i);
                }
            });
        }
        "set" => {
            let i = index_of(req, current.len(), "filters")?;
            let f = build_filter(item_object(req)?, Some(&current[i]))?;
            editor_edit(app, "Filter Settings", &move |l| {
                if let Some(slot) = l.get_mut(i) {
                    *slot = f.clone();
                }
            });
        }
        _ => {
            let i = index_of(req, current.len(), "filters")?;
            let to = req
                .to
                .and_then(|v| v.as_f64())
                .filter(|n| *n >= 0.0 && n.fract() == 0.0)
                .map(|n| n as usize)
                .filter(|t| *t < current.len())
                .ok_or_else(|| format!("op move needs 'to' as a position 0..{}", current.len() - 1))?;
            if to == i {
                return Err("the filter is already there".into());
            }
            // The panel's reorder swaps neighbours; a longer move is that, repeated.
            editor_edit(app, "Reorder Filters", &move |l| {
                if i < to {
                    for k in i..to {
                        l.swap(k, k + 1);
                    }
                } else {
                    for k in (to..i).rev() {
                        l.swap(k, k + 1);
                    }
                }
            });
        }
    }
    Ok(Value::Null)
}

// -- modifiers --------------------------------------------------------------------------

const MODIFIER_KINDS: &[&str] =
    &["wiggle", "spring", "look_at", "squash_stretch", "breathe", "blink", "turn", "sway", "drift"];

fn modifier_fields(m: &Modifier) -> (&'static str, Vec<(&'static str, f64)>) {
    match *m {
        Modifier::Wiggle { amplitude, frequency } => {
            ("wiggle", vec![("amplitude", amplitude), ("frequency", frequency)])
        }
        Modifier::Spring { root, stiffness, damping, coupling } => (
            "spring",
            vec![("root", root as f64), ("stiffness", stiffness), ("damping", damping), ("coupling", coupling)],
        ),
        Modifier::LookAt { x, y } => ("look_at", vec![("x", x), ("y", y)]),
        Modifier::AutoSquashStretch { amount } => ("squash_stretch", vec![("amount", amount)]),
        Modifier::Breathe { rate, depth } => ("breathe", vec![("rate", rate), ("depth", depth)]),
        Modifier::Blink { rate, duration } => ("blink", vec![("rate", rate), ("duration", duration)]),
        Modifier::Turn { round } => ("turn", vec![("round", round)]),
        Modifier::Sway { amount, rate } => ("sway", vec![("amount", amount), ("rate", rate)]),
        Modifier::Drift { dx, dy, span, phase } => {
            ("drift", vec![("dx", dx), ("dy", dy), ("span", span), ("phase", phase)])
        }
    }
}

/// The panel's own starting values for each kind (and the dialogs' for the
/// wiggle and spring).
fn modifier_default(kind: &str) -> Option<Modifier> {
    Some(match norm(kind).as_str() {
        "wiggle" => Modifier::Wiggle { amplitude: 12.0, frequency: 1.5 },
        "spring" => Modifier::Spring { root: 0, stiffness: 120.0, damping: 12.0, coupling: 0.012 },
        "lookat" => Modifier::LookAt { x: 0.0, y: 0.0 },
        "squashstretch" | "autosquashstretch" => Modifier::AutoSquashStretch { amount: 0.01 },
        "breathe" => Modifier::Breathe { rate: 14.0, depth: 1.0 },
        "blink" => Modifier::Blink { rate: 12.0, duration: 0.16 },
        "turn" => Modifier::Turn { round: 1.0 },
        "sway" => Modifier::Sway { amount: 0.12, rate: 0.2 },
        "drift" => Modifier::Drift { dx: 12.0, dy: 0.0, span: 0.0, phase: 0.0 },
        _ => return None,
    })
}

fn modifier_json(index: usize, m: &Modifier) -> Value {
    let (kind, fields) = modifier_fields(m);
    let mut out = Map::new();
    out.insert("index".into(), json!(index));
    out.insert("kind".into(), json!(kind));
    for (k, v) in fields {
        out.insert(k.into(), json!(v));
    }
    Value::Object(out)
}

fn modifiers_of(scene: &Scene, target: &Target) -> Result<Vec<Modifier>, String> {
    match target {
        Target::Element(id) => scene
            .find_object(*id)
            .map(|(_, o)| o.modifiers.clone())
            .ok_or_else(|| format!("no element {}", id.0)),
        _ => Err("list 'modifiers' lives on element:<id>".into()),
    }
}

fn modifiers_items(scene: &Scene, target: &Target) -> Result<Value, String> {
    let list = modifiers_of(scene, target)?;
    Ok(Value::Array(list.iter().enumerate().map(|(i, m)| modifier_json(i, m)).collect()))
}

/// A modifier from `item`, over `base` (setting) or the kind's defaults (adding).
fn build_modifier(item: &Map<String, Value>, base: Option<&Modifier>) -> Result<Modifier, String> {
    let wanted = item.get("kind").and_then(|v| v.as_str());
    let start = match (base, wanted) {
        (Some(b), None) => *b,
        (_, Some(k)) => {
            let fresh = modifier_default(k)
                .ok_or_else(|| format!("unknown modifier kind '{k}'; valid: {}", MODIFIER_KINDS.join(", ")))?;
            match base {
                // Same kind: keep its values; a different kind starts from defaults.
                Some(b) if modifier_fields(b).0 == modifier_fields(&fresh).0 => *b,
                _ => fresh,
            }
        }
        (None, None) => return Err(format!("item needs a 'kind'; valid: {}", MODIFIER_KINDS.join(", "))),
    };
    let (kind, mut fields) = modifier_fields(&start);
    for (key, val) in item {
        if key == "kind" {
            continue;
        }
        let Some(slot) = fields.iter_mut().find(|(k, _)| k == key) else {
            let valid: Vec<&str> = fields.iter().map(|(k, _)| *k).collect();
            return Err(format!("unknown parameter '{key}' for {kind}; valid: kind, {}", valid.join(", ")));
        };
        let n = val
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| format!("'{key}' needs a number"))?;
        slot.1 = n;
    }
    let f = |name: &str| fields.iter().find(|(k, _)| *k == name).map(|(_, v)| *v).unwrap_or(0.0);
    Ok(match kind {
        "wiggle" => Modifier::Wiggle { amplitude: f("amplitude").max(0.0), frequency: f("frequency").max(0.0) },
        "spring" => {
            let root = f("root");
            if root < 0.0 || root.fract() != 0.0 {
                return Err("'root' needs a bone index (a whole number >= 0)".into());
            }
            Modifier::Spring {
                root: root as usize,
                stiffness: f("stiffness"),
                damping: f("damping"),
                coupling: f("coupling"),
            }
        }
        "look_at" => Modifier::LookAt { x: f("x"), y: f("y") },
        "squash_stretch" => Modifier::AutoSquashStretch { amount: f("amount") },
        "breathe" => Modifier::Breathe { rate: f("rate"), depth: f("depth") },
        "blink" => Modifier::Blink { rate: f("rate"), duration: f("duration") },
        "turn" => Modifier::Turn { round: f("round") },
        "sway" => Modifier::Sway { amount: f("amount"), rate: f("rate") },
        _ => Modifier::Drift { dx: f("dx"), dy: f("dy"), span: f("span"), phase: f("phase") },
    })
}

fn modifiers_apply(app: &mut App, target: &Target, req: &Req) -> Result<Value, String> {
    let scene = app.agent_editor().doc.scene().clone();
    let current = modifiers_of(&scene, target)?;
    let Target::Element(id) = target else { unreachable!("modifiers_of refused it") };
    let id = *id;
    match req.op {
        "get" => {}
        "add" => {
            let m = build_modifier(item_object(req)?, None)?;
            if let Modifier::Spring { root, .. } = m {
                let (_, armature) = armature_of(&scene, target, "modifiers (spring)")?;
                if root >= armature.bones.len() {
                    return Err(format!("spring 'root' {root} is not a bone; the rig has {}", armature.bones.len()));
                }
            }
            app.agent_editor_mut().add_modifier(id, m);
        }
        "remove" => {
            let i = index_of(req, current.len(), "modifiers")?;
            app.agent_editor_mut().remove_modifier(id, i);
        }
        _ => {
            let i = index_of(req, current.len(), "modifiers")?;
            let m = build_modifier(item_object(req)?, Some(&current[i]))?;
            app.agent_editor_mut().set_modifier(id, i, m);
        }
    }
    Ok(Value::Null)
}

// -- swatches -----------------------------------------------------------------------------

fn swatch_items(scene: &Scene) -> Value {
    Value::Array(
        scene
            .swatches()
            .iter()
            .enumerate()
            .map(|(index, s)| {
                json!({"index": index, "id": s.id.0, "name": s.name, "color": hex_of(s.color), "folder": s.folder})
            })
            .collect(),
    )
}

fn folder_items(scene: &Scene) -> Value {
    Value::Array(
        scene
            .swatches()
            .folders()
            .enumerate()
            .map(|(index, f)| json!({"index": index, "path": f}))
            .collect(),
    )
}

fn document_only(target: &Target, list: &str) -> Result<(), String> {
    if matches!(target, Target::Document) {
        Ok(())
    } else {
        Err(format!("list '{list}' lives on the target 'document'"))
    }
}

/// The swatch a call names: `index`, or `item.id`.
fn swatch_named(scene: &Scene, req: &Req) -> Result<SwatchId, String> {
    let all: Vec<SwatchId> = scene.swatches().iter().map(|s| s.id).collect();
    if let Some(id) = req.item.and_then(|i| i.get("id")).and_then(|v| v.as_u64()) {
        return all
            .iter()
            .copied()
            .find(|s| s.0 == id)
            .ok_or_else(|| format!("no swatch with id {id}"));
    }
    Ok(all[index_of(req, all.len(), "swatches")?])
}

/// A folder path from `to` / `item.folder`: text, or null / "" for the root.
fn folder_arg(v: Option<&Value>) -> Result<Option<String>, String> {
    match v {
        None => Err("needs a folder (text), or null for the root".into()),
        Some(Value::Null) => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or("a folder is text, or null for the root")?.trim().trim_matches('/');
            Ok((!s.is_empty()).then(|| s.to_string()))
        }
    }
}

fn swatches_apply(app: &mut App, target: &Target, req: &Req) -> Result<Value, String> {
    document_only(target, "swatches")?;
    let scene = app.agent_editor().doc.scene().clone();
    let mut extra = json!({});
    match req.op {
        "get" => {}
        "add" => {
            let item = item_object(req)?;
            unknown_keys(item, &["name", "color", "folder"])?;
            let color = item
                .get("color")
                .and_then(|v| v.as_str())
                .and_then(parse_hex)
                .ok_or("add needs item.color like \"#rrggbb\"")?;
            let name = match item.get("name") {
                None => "Swatch".to_string(),
                Some(v) => v.as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or("'name' needs non-empty text")?.to_string(),
            };
            let folder = match item.get("folder") {
                None => None,
                some => folder_arg(some)?,
            };
            app.agent_editor_mut().doc.edit("Swatches", |scene| {
                scene.add_swatch(&name, color, folder);
            });
        }
        "remove" => {
            let id = swatch_named(&scene, req)?;
            app.agent_editor_mut().doc.edit("Swatches", |scene| {
                scene.swatches_mut().remove(id);
            });
        }
        "move" => {
            let id = swatch_named(&scene, req)?;
            let folder = folder_arg(req.to)?;
            app.agent_editor_mut().doc.edit("Swatches", |scene| {
                scene.swatches_mut().update(id, |s| s.folder = folder);
            });
        }
        _ => {
            let id = swatch_named(&scene, req)?;
            let item = item_object(req)?;
            unknown_keys(item, &["id", "name", "color", "folder"])?;
            if !["name", "color", "folder"].iter().any(|k| item.contains_key(*k)) {
                return Err("set needs item.name, item.color and/or item.folder".into());
            }
            let name = match item.get("name") {
                None => None,
                Some(v) => Some(v.as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or("'name' needs non-empty text")?.to_string()),
            };
            let color = match item.get("color") {
                None => None,
                Some(v) => Some(v.as_str().and_then(parse_hex).ok_or("'color' needs a colour like \"#rrggbb\"")?),
            };
            let folder = match item.get("folder") {
                None => None,
                some => Some(folder_arg(some)?),
            };
            let mut repainted = 0usize;
            app.agent_editor_mut().doc.edit("Swatches", |scene| {
                if let Some(name) = name {
                    scene.swatches_mut().update(id, |s| s.name = name);
                }
                if let Some(folder) = folder {
                    scene.swatches_mut().update(id, |s| s.folder = folder);
                }
                // Changing the colour repaints every fill and stroke linked to it.
                if let Some(color) = color {
                    repainted = scene.recolour_swatch(id, color);
                }
            });
            extra = json!({"repainted": repainted});
        }
    }
    Ok(extra)
}

fn folders_apply(app: &mut App, target: &Target, req: &Req) -> Result<Value, String> {
    document_only(target, "swatch_folders")?;
    let scene = app.agent_editor().doc.scene().clone();
    let folders: Vec<String> = scene.swatches().folders().cloned().collect();
    match req.op {
        "get" => {}
        "add" => {
            let item = item_object(req)?;
            unknown_keys(item, &["name"])?;
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().trim_matches('/').to_string())
                .filter(|s| !s.is_empty())
                .ok_or("add needs item.name (a folder path such as \"Hero/Skin\")")?;
            if folders.contains(&name) {
                return Err(format!("the folder '{name}' already exists"));
            }
            app.agent_editor_mut().doc.edit("Swatches", |scene| {
                scene.swatches_mut().add_folder(&name);
            });
        }
        _ => {
            let path = match req.item.and_then(|i| i.get("name")).and_then(|v| v.as_str()) {
                Some(name) => folders
                    .iter()
                    .find(|f| *f == name)
                    .cloned()
                    .ok_or_else(|| format!("no folder '{name}'; folders: {}", folders.join(", ")))?,
                None => folders[index_of(req, folders.len(), "swatch_folders")?].clone(),
            };
            // Its colours are kept, moved to the root.
            app.agent_editor_mut().doc.edit("Swatches", |scene| {
                scene.swatches_mut().remove_folder(&path);
            });
        }
    }
    Ok(Value::Null)
}

// -- the rig ---------------------------------------------------------------------------------

fn poses_items(scene: &Scene, target: &Target) -> Result<Value, String> {
    let Target::Element(id) = target else {
        return Err("list 'poses' lives on element:<id> of an armature".into());
    };
    match scene.find_object(*id).map(|(_, o)| &o.kind) {
        Some(ObjectKind::Armature(rig)) => Ok(Value::Array(
            rig.poses
                .iter()
                .enumerate()
                .map(|(i, p)| json!({"index": i, "name": p.name, "bones": p.angles.len()}))
                .collect(),
        )),
        _ => Err("not an armature".into()),
    }
}

fn bones_items(scene: &Scene, target: &Target) -> Result<Value, String> {
    let (_, armature) = armature_of(scene, target, "bones")?;
    Ok(Value::Array(
        armature
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| {
                json!({"index": i, "name": b.name, "parent": b.parent, "pinned": b.pinned, "limited": b.limits.is_some()})
            })
            .collect(),
    ))
}

fn poses_apply(app: &mut App, target: &Target, req: &Req) -> Result<Value, String> {
    let scene = app.agent_editor().doc.scene().clone();
    let (object, _) = armature_of(&scene, target, "poses")?;
    let saved = match poses_items(&scene, target)? {
        Value::Array(list) => list.len(),
        _ => 0,
    };
    match req.op {
        "get" => {}
        "add" => {
            let item = item_object(req)?;
            unknown_keys(item, &["name"])?;
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .ok_or("add needs item.name: the name to save the current pose under")?
                .to_string();
            app.agent_editor_mut().rig_save_pose(object, name);
        }
        "set" => {
            let i = index_of(req, saved, "poses")?;
            let key = match req.item {
                None => false,
                Some(v) => {
                    let obj = v.as_object().ok_or("'item' needs an object like {key: true}")?;
                    unknown_keys(obj, &["key"])?;
                    obj.get("key").map(|k| k.as_bool().ok_or("'key' needs true or false")).transpose()?.unwrap_or(false)
                }
            };
            app.agent_editor_mut().apply_named_pose(object, i, key);
        }
        _ => {
            let i = index_of(req, saved, "poses")?;
            app.agent_editor_mut().rig_delete_pose(object, i);
        }
    }
    Ok(Value::Null)
}

fn bones_apply(app: &mut App, target: &Target, req: &Req) -> Result<Value, String> {
    let scene = app.agent_editor().doc.scene().clone();
    let (object, armature) = armature_of(&scene, target, "bones")?;
    match req.op {
        "get" => {}
        "remove" => {
            let i = index_of(req, armature.bones.len(), "bones")?;
            let mut trial = armature.clone();
            if !trial.remove_bone(i) {
                return Err(format!("bone {i} cannot be deleted"));
            }
            app.agent_editor_mut().rig_delete_bone(object, i);
        }
        _ => {
            let i = index_of(req, armature.bones.len(), "bones")?;
            let parent = match req.to {
                None => return Err("op move needs 'to': the new parent bone index, or null to make it a root".into()),
                Some(Value::Null) => None,
                Some(v) => Some(
                    v.as_f64()
                        .filter(|n| *n >= 0.0 && n.fract() == 0.0 && (*n as usize) < armature.bones.len())
                        .map(|n| n as usize)
                        .ok_or_else(|| format!("'to' needs a bone index 0..{}, or null", armature.bones.len() - 1))?,
                ),
            };
            let mut trial = armature.clone();
            if !trial.reparent_bone(i, parent) {
                return Err("A bone cannot hang off itself or off one of its own children".into());
            }
            app.agent_editor_mut().rig_reparent_bone(object, i, parent);
        }
    }
    Ok(Value::Null)
}

fn rig_apply(app: &mut App, target: &Target, req: &Req) -> Result<Value, String> {
    let scene = app.agent_editor().doc.scene().clone();
    let (object, _) = armature_of(&scene, target, "rig")?;
    let editor = app.agent_editor_mut();
    match req.op {
        "get" => {}
        "set_rest_pose" => editor.rig_set_rest_pose(object),
        "mirror_pose" => editor.rig_mirror_pose(object),
        _ => editor.rig_reset_pose(object),
    }
    Ok(Value::Null)
}
