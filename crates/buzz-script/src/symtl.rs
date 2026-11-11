//! What `characterSetup.jsfl` and its neighbours needed on top of the shelf
//! (JSFL_COMMANDS_PLAN J6).
//!
//! * a **library item's own timeline** -- `libraryItem.timeline.layers[i].frames[f]
//!   .elements[j]`, with layer names that can be written -- so a script can read
//!   and rename the layers inside a symbol it has not entered;
//! * an element's **bounds** (`left`, `top`, `width`, `height`);
//! * `document.breakApart()`, `document.traceBitmap()` and `document.importFile()`;
//! * the **dialogs**: `confirm`, `fl.xmlPanel`, `fl.browseForFileURL`, asked of a
//!   person through [`crate::Hooks::ask`] when the host has one.
//!
//! Nothing here opens a file by itself: `importFile` reads only a file the
//! person picked in a dialog during the run, or one inside the script folders.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use buzz_geom::Point;
use buzz_scene::{LayerKind, ObjectKind, Scene};
use buzz_scene::Object as SceneObject;
use rquickjs::{Ctx, Function, Object, Result as JsResult};

use crate::host::{
    host_fn, layer_kind_str, locate_element, target, throw, tween_kind_str, uri_to_path,
};
use crate::{Answer, Question, State};

/// `file:///C|/dir/name.png`, the way a script is handed a picked file.
fn file_uri(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = match text.as_bytes() {
        [drive, b':', ..] => format!("{}|{}", *drive as char, &text[2..]),
        _ => text,
    };
    format!("file:///{text}")
}

/// One element, as JSON, for a script reading a symbol it has not entered.
fn element_json(scene: &Scene, obj: &SceneObject) -> serde_json::Value {
    let c = obj.transform.as_coeffs();
    let (element_type, symbol_type, lib_name, lib_type) = match &obj.kind {
        ObjectKind::Instance(inst) => {
            let symbol = scene.library().get(inst.symbol);
            let kind = symbol.map(|sy| crate::host::symbol_kind_str(sy.kind)).unwrap_or("");
            ("instance", kind, symbol.map(|sy| sy.name.clone()).unwrap_or_default(), kind)
        }
        ObjectKind::Group(_) => ("group", "", String::new(), ""),
        _ => ("shape", "", String::new(), ""),
    };
    let b = scene.resolved_bounds(obj);
    serde_json::json!({
        "elementType": element_type,
        "symbolType": symbol_type,
        "libraryItemName": lib_name,
        "libraryItemType": lib_type,
        "name": obj.name.clone().unwrap_or_default(),
        "x": c[4], "y": c[5],
        "a": c[0], "b": c[1], "c": c[2], "d": c[3], "tx": c[4], "ty": c[5],
        "left": b.x0, "top": b.y0, "width": b.width(), "height": b.height(),
    })
}

/// The top-left, centre or bottom-centre... of `rect` that a registration name
/// (`"bottom center"`, `"top left"`, `"center"`) means.
pub(crate) fn registration_point(name: &str, rect: kurbo::Rect) -> Option<Point> {
    let n = name.to_ascii_lowercase();
    if n.trim().is_empty() {
        return None;
    }
    let x = if n.contains("left") {
        rect.x0
    } else if n.contains("right") {
        rect.x1
    } else {
        (rect.x0 + rect.x1) / 2.0
    };
    let y = if n.contains("top") {
        rect.y0
    } else if n.contains("bottom") {
        rect.y1
    } else {
        (rect.y0 + rect.y1) / 2.0
    };
    Some(Point::new(x, y))
}


/// Ask the host's dialog hook, if there is one, without holding the state while
/// a person thinks. The time spent does not count against the script.
pub(crate) fn ask(state: &Rc<RefCell<State>>, question: Question) -> Option<Answer> {
    let (hook, waited) = {
        let s = state.borrow();
        (s.context.hooks.ask.clone()?, Arc::clone(&s.waited))
    };
    let started = Instant::now();
    let answer = hook(question);
    waited.fetch_add(
        started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
    Some(answer)
}

pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    host: &Object<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    // -- a library item's own timeline ---------------------------------------
    host_fn!(ctx, host, state, "symLayers", |state, name: String| {
        let s = state.borrow();
        let Some(sym) = s.scene().library().find_by_name(&name) else {
            return Err(throw(&format!("there is no library item called {name:?}")));
        };
        let layers: Vec<serde_json::Value> = sym
            .layers
            .iter()
            .map(|l| {
                serde_json::json!({
                    "name": l.name,
                    "layerType": layer_kind_str(l.kind),
                    "locked": l.locked,
                    "visible": l.visible,
                    "frameCount": l.frames.length(),
                })
            })
            .collect();
        Ok(serde_json::Value::Array(layers).to_string())
    });
    host_fn!(
        ctx,
        host,
        state,
        "symSetLayer",
        |state, name: String, index: i32, key: String, value: String| {
            let mut s = state.borrow_mut();
            let Some(sym) = s.scene().library().find_by_name(&name).map(|x| x.id) else {
                return Err(throw(&format!("there is no library item called {name:?}")));
            };
            let Some(layer) = (index >= 0)
                .then(|| s.scene().library().get(sym))
                .flatten()
                .and_then(|x| x.layers.iter().nth(index as usize).map(|l| l.id))
            else {
                return Err(throw(&format!("{name:?} has no layer {index}")));
            };
            let on = value == "true";
            let changed = s.scene_mut().library_mut().update(sym, |sy| {
                sy.layers.update(layer, |l| match key.as_str() {
                    "name" => l.name = value.clone(),
                    "locked" => l.locked = on,
                    "visible" => l.visible = on,
                    _ => {}
                });
            });
            Ok(changed)
        }
    );
    host_fn!(
        ctx,
        host,
        state,
        "symFrame",
        |state, name: String, layer: i32, frame: u32| {
            let s = state.borrow();
            let Some(sym) = s.scene().library().find_by_name(&name) else {
                return Err(throw(&format!("there is no library item called {name:?}")));
            };
            let Some(l) = (layer >= 0).then(|| sym.layers.iter().nth(layer as usize)).flatten()
            else {
                return Err(throw(&format!("{name:?} has no layer {layer}")));
            };
            let start = l.frames.keyframe_start(frame).unwrap_or(frame);
            let next = l
                .frames
                .keyframes()
                .iter()
                .map(|k| k.start)
                .find(|s2| *s2 > start)
                .unwrap_or(l.frames.length());
            let elements: Vec<serde_json::Value> = l
                .frames
                .objects_at(frame)
                .iter()
                .map(|o| element_json(s.scene(), o))
                .collect();
            Ok(serde_json::json!({
                "startFrame": start,
                "duration": next.saturating_sub(start),
                "isKeyframe": l.frames.is_keyframe(frame),
                "tweenType": tween_kind_str(l.frames.tween_at(frame).kind),
                "elements": elements,
            })
            .to_string())
        }
    );

    // -- an element's bounds --------------------------------------------------
    host_fn!(ctx, host, state, "elementBounds", |state, id: u64, frame: u32| {
        let s = state.borrow();
        let obj = locate_element(&s, id, frame)?;
        let b = s.scene().resolved_bounds(&obj);
        Ok(serde_json::json!([b.x0, b.y0, b.x1, b.y1]).to_string())
    });

    // -- a new layer, above or below the one that is selected ----------------
    host_fn!(
        ctx,
        host,
        state,
        "addNewLayerEx",
        |state, name: String, kind: String, above: bool| {
            let mut s = state.borrow_mut();
            let count = s.scene().layers().len();
            let name = if name.is_empty() { format!("Layer_{}", count + 1) } else { name };
            let kind = match kind.to_ascii_lowercase().as_str() {
                "guide" => LayerKind::Guide,
                "folder" => LayerKind::Folder,
                "mask" => LayerKind::Mask,
                "masked" => LayerKind::Masked,
                "guided" => LayerKind::Guided,
                _ => LayerKind::Normal,
            };
            let reference = s.selected_layer.filter(|i| *i >= 0 && (*i as usize) < count);
            let id = s.scene_mut().add_layer(name, kind);
            // The new layer is at the front; the layer it was added next to has
            // moved down one. Above that layer is its old row, below is the next.
            let to = match reference {
                None => 0,
                Some(r) if above => r as usize,
                Some(r) => r as usize + 1,
            };
            if to > 0 {
                s.scene_mut().reorder_layer(id, to);
            }
            // Animate makes the new layer the current one.
            s.selected_layer = Some(to as i32);
            Ok(to as i32)
        }
    );

    // -- Modify > Break Apart -------------------------------------------------
    host_fn!(ctx, host, state, "breakApart", |state| {
        let ids = state.borrow().context.selection.clone();
        if ids.is_empty() {
            return Err(throw("select something to break apart"));
        }
        let frame = state.borrow().context.current_frame;
        let mut s = state.borrow_mut();
        let mut fresh = Vec::new();
        for id in ids {
            let Some((layer, obj)) = s.scene().find_object(id).map(|(l, o)| (l, o.clone())) else {
                continue;
            };
            let pieces: Vec<SceneObject> = match &obj.kind {
                ObjectKind::Instance(inst) => {
                    let Some(sym) = s.scene().library().get(inst.symbol).cloned() else {
                        continue;
                    };
                    let start = s
                        .scene()
                        .layers()
                        .get(layer)
                        .and_then(|l| l.frames.keyframe_start(frame))
                        .unwrap_or(frame);
                    let shown = inst.resolve_frame(sym.kind, frame.saturating_sub(start), sym.length());
                    let mut out = Vec::new();
                    for l in sym.layers.drawable_at(shown) {
                        if l.kind.is_mask() || matches!(l.kind, LayerKind::Guide | LayerKind::Folder) {
                            continue;
                        }
                        let follows = sym.layers.inherited_transform(l.id, shown);
                        let resolved = sym.layers.resolved_at_rigged(l.id, shown);
                        for o in resolved.iter() {
                            let mut piece = o.clone();
                            piece.transform = obj.transform * follows * piece.transform;
                            out.push(piece);
                        }
                    }
                    out
                }
                ObjectKind::Group(children) => children
                    .iter()
                    .map(|c| {
                        let mut piece = (**c).clone();
                        piece.transform = obj.transform * piece.transform;
                        piece
                    })
                    .collect(),
                // A drawing is already broken apart; so is an imported picture.
                _ => {
                    fresh.push(id);
                    continue;
                }
            };
            s.scene_mut().remove_object(id);
            for mut piece in pieces {
                piece.id = s.scene_mut().next_object_id();
                let new = piece.id;
                if s.scene_mut().add_object_at(layer, frame, piece).is_some() {
                    fresh.push(new);
                }
            }
        }
        s.context.selection = fresh;
        Ok(())
    });

    // -- Modify > Bitmap > Trace Bitmap ---------------------------------------
    host_fn!(ctx, host, state, "traceBitmap", |state| {
        let Some(hook) = state.borrow().context.hooks.trace.clone() else {
            return Err(throw("Trace Bitmap is not available where this script is running"));
        };
        let ids = state.borrow().context.selection.clone();
        if ids.is_empty() {
            return Err(throw("select a bitmap to trace"));
        }
        let frame = state.borrow().context.current_frame;
        let mut s = state.borrow_mut();
        let Some(layer) = ids.iter().find_map(|id| s.scene().find_object(*id).map(|(l, _)| l)) else {
            return Err(throw("the selected bitmap is not on this timeline"));
        };
        let made = hook(s.scene_mut(), layer, frame, &ids).map_err(|m| throw(&m))?;
        s.context.selection = made;
        Ok(())
    });

    // -- File > Import ---------------------------------------------------------
    host_fn!(ctx, host, state, "importFile", |state, uri: String| {
        let path = uri_to_path(&uri)?;
        let real = path
            .canonicalize()
            .map_err(|e| throw(&format!("could not open {uri}: {e}")))?;
        {
            // The sandbox: a file the person picked in a dialog during this run,
            // or one in the script or asset folders. Nothing else on the disk.
            let s = state.borrow();
            let inside = |root: &Option<std::path::PathBuf>| {
                root.as_ref()
                    .and_then(|r| r.canonicalize().ok())
                    .is_some_and(|r| real.starts_with(r))
            };
            let granted = s
                .granted_files
                .iter()
                .any(|g| g.canonicalize().is_ok_and(|g| g == real));
            if !granted && !inside(&s.context.config_dir) && !inside(&s.context.asset_root) {
                return Err(throw(&format!(
                    "{uri} is not a file this script may read; pick it with fl.browseForFileURL"
                )));
            }
        }
        let bytes = std::fs::read(&real).map_err(|e| throw(&format!("could not read {uri}: {e}")))?;
        let name = real
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Bitmap".to_string());
        let (layer, frame) = target(&state.borrow())?;
        let mut s = state.borrow_mut();
        // A frame past the layer's last span has no keyframe to hold the picture;
        // Animate makes one, so do the same.
        s.scene_mut().update_layer(layer, |l| {
            if l.frames.keyframe_at(frame).is_none() {
                l.frames.insert_blank_keyframe(frame);
            }
        });
        let stage = s.scene().stage().size;
        let asset = s
            .scene_mut()
            .add_image(&name, &bytes)
            .map_err(|e| throw(&format!("{uri} is not a picture: {e}")))?;
        // A picture larger than the stage is scaled to fit it, as Import Image does.
        let (w, h) = (asset.width as f64, asset.height as f64);
        let fit = (stage.width / w).min(stage.height / h).min(1.0);
        let (w, h) = (w * fit, h * fit);
        let (cx, cy) = (stage.width / 2.0, stage.height / 2.0);
        let rect = kurbo::Rect::new(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0);
        match s.scene_mut().place_image_in(layer, frame, asset, rect) {
            Some(id) => {
                s.context.selection = vec![id];
                Ok(true)
            }
            None => Err(throw("could not place the picture; the layer may be locked")),
        }
    });

    // -- the dialogs ---------------------------------------------------------
    host_fn!(ctx, host, state, "confirmAsk", |state, message: String| {
        // Unattended, yes: these are the user's own commands, run on purpose.
        let yes = match ask(state, Question::Confirm { message: message.clone() }) {
            None => true,
            Some(Answer::Yes(b)) => b,
            Some(_) => false,
        };
        state.borrow_mut().alerts.push(format!(
            "{message} - answered {:?}",
            if yes { "yes" } else { "no" }
        ));
        Ok(yes)
    });
    host_fn!(ctx, host, state, "xmlPanel", |state, xml: String| {
        let Some(form) = crate::xmlui::parse(&xml) else {
            return Err(throw("that is not an XMLUI dialog: it has no <dialog> element"));
        };
        let result = match ask(state, Question::Form(form.clone())) {
            None => crate::xmlui::result_json(Some(&form.defaults())),
            Some(Answer::Form(values)) => crate::xmlui::result_json(Some(&values)),
            Some(_) => crate::xmlui::result_json(None),
        };
        state
            .borrow_mut()
            .alerts
            .push(format!("dialog {:?} - answered {result}", form.title));
        Ok(result)
    });
    host_fn!(ctx, host, state, "browseFile", |state, save: bool, title: String| {
        match ask(state, Question::PickFile { title, save }) {
            Some(Answer::File(path)) => {
                let uri = file_uri(&path);
                state.borrow_mut().granted_files.push(path);
                Ok(uri)
            }
            // Cancelled, or nobody to ask: the script sees no file.
            _ => Ok(String::new()),
        }
    });

    Ok(())
}
