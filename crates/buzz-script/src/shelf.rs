//! The rest of the JSFL a shelf of Animate commands calls (JSFL_COMMANDS_PLAN J2).
//!
//! `host.rs` holds the first command-shelf subset (frames, keyframes, tweens,
//! entering a symbol). This is what the user's own Commands folder needed on
//! top of that: layer parenting (`getRigParentAtFrame` / `setRigParentAtFrame`),
//! the transform half of an element (`rotation`, `scaleX`, ...), library items
//! you can swap, rename and enter, instance tint, a layer's filters and colour
//! transform, and a file store for the little data files commands keep
//! (`FLfile`).
//!
//! # The file store
//!
//! A command that remembers something (`LayerParenting.txt` beside the
//! Commands folder) writes through `FLfile`. Writes never go to Animate's own
//! Configuration folder -- that is the user's installed Animate, and a
//! BuzzAnimate script has no business editing it. They go to an **overlay**
//! under BuzzAnimate's own data folder, laid out like the Configuration folder,
//! and a read looks in the overlay first and then in Animate's. A script
//! therefore reads back what it wrote, and reads the commands it ships with.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use buzz_geom::Affine;
use buzz_scene::{ObjectId, ObjectKind};
use peniko::Color;
use rquickjs::{Ctx, Function, Object, Result as JsResult};

use crate::State;
use crate::host::{host_fn, layer_id, throw};

/// Where scripts' own files are written: BuzzAnimate's data folder, never
/// Animate's Configuration folder.
fn overlay_root() -> PathBuf {
    let base = std::env::var_os("BUZZ_SCRIPT_DATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("BuzzAnimate").join("ScriptData"))
        })
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(|a| PathBuf::from(a).join("BuzzAnimate").join("ScriptData"))
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".config").join("BuzzAnimate").join("ScriptData"))
        })
        .unwrap_or_else(|| std::env::temp_dir().join("BuzzAnimate").join("ScriptData"));
    base
}

/// `file:///C|/x/y` or a plain path, to a path.
fn uri_path(uri: &str) -> PathBuf {
    let text = uri.trim();
    let text = text
        .strip_prefix("file:///")
        .or_else(|| text.strip_prefix("file://"))
        .unwrap_or(text);
    let text = text.replace("%20", " ");
    let text = match text.as_bytes() {
        [drive, b'|', ..] => format!("{}:{}", *drive as char, &text[2..]),
        _ => text,
    };
    PathBuf::from(text)
}

fn lexical(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_ascii_lowercase()
}

/// Where a script's read of `uri` is served from, and where its write goes.
/// `None` for a path outside every folder a script may use.
struct Resolved {
    read: Vec<PathBuf>,
    write: Option<PathBuf>,
}

fn resolve(state: &State, uri: &str) -> Option<Resolved> {
    let path = uri_path(uri);
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return None;
    }
    if let Some(config) = &state.context.config_dir {
        let (p, c) = (lexical(&path), lexical(config));
        if p == c || p.starts_with(&format!("{c}/")) {
            let rel = path.to_string_lossy().replace('\\', "/");
            let rel_len = config.to_string_lossy().trim_end_matches(['/', '\\']).len();
            let rel = rel.get(rel_len..).unwrap_or("").trim_start_matches('/').to_string();
            let overlay = overlay_root().join(&rel);
            return Some(Resolved {
                read: vec![overlay.clone(), path.clone()],
                write: Some(overlay),
            });
        }
    }
    if let Some(root) = &state.context.asset_root {
        let (p, r) = (lexical(&path), lexical(root));
        if p == r || p.starts_with(&format!("{r}/")) {
            return Some(Resolved { read: vec![path.clone()], write: Some(path) });
        }
    }
    let overlay = overlay_root();
    let (p, o) = (lexical(&path), lexical(&overlay));
    if p == o || p.starts_with(&format!("{o}/")) {
        return Some(Resolved { read: vec![path.clone()], write: Some(path) });
    }
    None
}

fn not_allowed(uri: &str) -> rquickjs::Error {
    throw(&format!(
        "{uri} is outside the folders a script may use (the Commands folder, the asset \
         library and BuzzAnimate's script data folder)"
    ))
}

/// Install the shelf calls onto `__host`.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    host: &Object<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    // -- layer parenting -----------------------------------------------------
    //
    // Animate's rig parent is a layer another layer follows; here that is
    // `Layer::follows`.
    host_fn!(ctx, host, state, "layerFollows", |state, index: i32| {
        let s = state.borrow();
        let id = layer_id(&s, index)?;
        let Some(parent) = s.scene().layers().get(id).and_then(|l| l.follows) else {
            return Ok(-1i32);
        };
        Ok(s.scene()
            .layers()
            .iter()
            .position(|x| x.id == parent)
            .map(|p| p as i32)
            .unwrap_or(-1))
    });
    host_fn!(
        ctx,
        host,
        state,
        "setLayerFollows",
        |state, index: i32, parent: i32, frame: u32| {
            let mut s = state.borrow_mut();
            let child = layer_id(&s, index)?;
            let parent = if parent < 0 { None } else { Some(layer_id(&s, parent)?) };
            Ok(s.scene_mut().set_follows(child, parent, frame))
        }
    );

    host_fn!(ctx, host, state, "currentLayerIndex", |state| {
        let s = state.borrow();
        if let Some(i) = s.selected_layer {
            return Ok(i);
        }
        let active = s.context.active_layer;
        Ok(active
            .and_then(|a| s.scene().layers().iter().position(|l| l.id == a))
            .map(|p| p as i32)
            .unwrap_or(0))
    });

    host_fn!(ctx, host, state, "sceneRevision", |state| {
        Ok(state.borrow().scene().revision() as f64)
    });

    // -- the transform half of an element -----------------------------------
    //
    // Animate's `rotation`, `scaleX`, `scaleY`, `skewX`, `skewY` are views of the
    // matrix: scale is the length of each column, rotation the angle of the
    // first. Setting one rebuilds the matrix keeping the transformation point
    // where it was on the stage, which is what dragging the rotation handle
    // does.
    host_fn!(
        ctx,
        host,
        state,
        "elementTransformParts",
        |state, id: u64, frame: u32| {
            let s = state.borrow();
            let obj = crate::host::locate_element(&s, id, frame)?;
            let [a, b, c, d, _, _] = obj.transform.as_coeffs();
            let scale_x = a.hypot(b);
            let det = a * d - b * c;
            let scale_y = if scale_x == 0.0 { 0.0 } else { det / scale_x };
            let rotation = b.atan2(a).to_degrees();
            let skew_y = rotation;
            let skew_x = (-c).atan2(d).to_degrees();
            let (loop_mode, name) = match &obj.kind {
                ObjectKind::Instance(i) => (
                    match i.loop_mode {
                        buzz_scene::LoopMode::Loop => "loop",
                        buzz_scene::LoopMode::PlayOnce => "play once",
                        buzz_scene::LoopMode::SingleFrame => "single frame",
                    },
                    obj.name.clone().unwrap_or_default(),
                ),
                _ => ("", obj.name.clone().unwrap_or_default()),
            };
            Ok(serde_json::json!({
                "scaleX": scale_x, "scaleY": scale_y, "rotation": rotation,
                "skewX": skew_x, "skewY": skew_y, "loop": loop_mode, "name": name,
            })
            .to_string())
        }
    );
    host_fn!(
        ctx,
        host,
        state,
        "setElementTransformParts",
        |state, id: u64, frame: u32, json: String| {
            let v: serde_json::Value =
                serde_json::from_str(&json).map_err(|_| throw("expected a transform object"))?;
            let mut s = state.borrow_mut();
            let current = crate::host::locate_element(&s, id, frame)?;
            let [a, b, c, d, tx, ty] = current.transform.as_coeffs();
            let pivot = current.pivot_local();
            let world_pivot = current.transform * kurbo::Point::new(pivot.x, pivot.y);

            let scale_x0 = a.hypot(b);
            let det = a * d - b * c;
            let scale_y0 = if scale_x0 == 0.0 { 1.0 } else { det / scale_x0 };
            let rot0 = b.atan2(a);
            let skew0 = (-c).atan2(d);
            let get = |k: &str| v.get(k).and_then(|x| x.as_f64());
            let scale_x = get("scaleX").unwrap_or(scale_x0);
            let scale_y = get("scaleY").unwrap_or(scale_y0);
            let (rot, skew) = match (get("rotation"), get("skewX"), get("skewY")) {
                (Some(r), _, _) => {
                    // Rotating turns the whole matrix: the skew between the axes is kept.
                    let delta = r.to_radians() - rot0;
                    (r.to_radians(), skew0 + delta)
                }
                (None, sx, sy) => (
                    sy.map(f64::to_radians).unwrap_or(rot0),
                    sx.map(f64::to_radians).unwrap_or(skew0),
                ),
            };
            // x axis at `rot`, y axis at `skew` measured the way `atan2(-c, d)` reads it.
            let na = scale_x * rot.cos();
            let nb = scale_x * rot.sin();
            let nc = -scale_y * skew.sin();
            let nd = scale_y * skew.cos();
            let mut m = Affine::new([na, nb, nc, nd, tx, ty]);
            // Keep the transformation point still.
            let moved = m * kurbo::Point::new(pivot.x, pivot.y);
            let co = m.as_coeffs();
            m = Affine::new([
                co[0],
                co[1],
                co[2],
                co[3],
                co[4] + world_pivot.x - moved.x,
                co[5] + world_pivot.y - moved.y,
            ]);
            let loop_mode = v.get("loop").and_then(|x| x.as_str()).map(str::to_string);
            let name = v.get("name").and_then(|x| x.as_str()).map(str::to_string);
            s.scene_mut().update_object_at(frame, ObjectId(id), |o| {
                o.transform = m;
                if let Some(n) = name {
                    o.name = if n.is_empty() { None } else { Some(n) };
                }
                if let (ObjectKind::Instance(i), Some(l)) = (&mut o.kind, loop_mode) {
                    i.loop_mode = match l.as_str() {
                        "play once" => buzz_scene::LoopMode::PlayOnce,
                        "single frame" => buzz_scene::LoopMode::SingleFrame,
                        _ => buzz_scene::LoopMode::Loop,
                    };
                }
            });
            Ok(())
        }
    );

    // -- document-level element commands ------------------------------------
    host_fn!(ctx, host, state, "resetTransformation", |state| {
        // Modify > Transform > Remove Transform: scale, rotation and skew go,
        // the position stays.
        let ids = state.borrow().context.selection.clone();
        let frame = state.borrow().context.current_frame;
        let mut s = state.borrow_mut();
        for id in ids {
            s.scene_mut().update_object_at(frame, id, |o| {
                let c = o.transform.as_coeffs();
                o.transform = Affine::new([1.0, 0.0, 0.0, 1.0, c[4], c[5]]);
            });
        }
        Ok(())
    });
    host_fn!(ctx, host, state, "swapElement", |state, name: String| {
        // Modify > Symbol > Swap Symbol on every selected instance.
        let ids = state.borrow().context.selection.clone();
        let frame = state.borrow().context.current_frame;
        let mut s = state.borrow_mut();
        let Some(symbol) = s.scene().library().find_by_name(&name).map(|x| x.id) else {
            return Err(throw(&format!("there is no library item called {name:?}")));
        };
        for id in ids {
            s.scene_mut().update_object_at(frame, id, |o| {
                if let ObjectKind::Instance(i) = &mut o.kind {
                    i.symbol = symbol;
                }
            });
        }
        Ok(())
    });
    host_fn!(
        ctx,
        host,
        state,
        "setTransformationPoint",
        |state, x: f64, y: f64| {
            let ids = state.borrow().context.selection.clone();
            let frame = state.borrow().context.current_frame;
            let mut s = state.borrow_mut();
            for id in ids {
                s.scene_mut().update_object_at(frame, id, |o| {
                    o.pivot = Some(kurbo::Point::new(x, y));
                });
            }
            Ok(())
        }
    );
    host_fn!(
        ctx,
        host,
        state,
        "setInstanceTint",
        |state, rgb: u32, percent: f64| {
            let ids = state.borrow().context.selection.clone();
            let frame = state.borrow().context.current_frame;
            let mut s = state.borrow_mut();
            let color = Color::from_rgba8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255);
            for id in ids {
                // A layer effect set earlier is re-based on this new tint.
                s.layer_fx_base.retain(|(object, _), _| *object != id.0);
                s.scene_mut().update_object_at(frame, id, |o| {
                    if let ObjectKind::Instance(i) = &mut o.kind {
                        i.color = buzz_scene::ColorTransform::tint(color, (percent / 100.0) as f32);
                    }
                });
            }
            Ok(())
        }
    );

    // -- the library ---------------------------------------------------------
    host_fn!(ctx, host, state, "libraryItems", |state| {
        let s = state.borrow();
        let items: Vec<serde_json::Value> = s
            .scene()
            .library()
            .iter()
            .map(|sy| {
                serde_json::json!({
                    "name": sy.name,
                    "itemType": crate::host::symbol_kind_str(sy.kind),
                    "frameCount": sy.layers.frame_count(),
                })
            })
            .collect();
        Ok(serde_json::Value::Array(items).to_string())
    });
    host_fn!(ctx, host, state, "libraryRename", |state, from: String, to: String| {
        let mut s = state.borrow_mut();
        let Some(id) = s.scene().library().find_by_name(&from).map(|x| x.id) else {
            return Ok(false);
        };
        Ok(s.scene_mut().library_mut().update(id, |sy| sy.name = to))
    });
    host_fn!(ctx, host, state, "libraryEdit", |state, name: String| {
        let mut s = state.borrow_mut();
        let Some(id) = s.scene().library().find_by_name(&name).map(|x| x.id) else {
            return Ok(false);
        };
        let entered = s.scene_mut().enter_symbol(id);
        // A different timeline: its layers and frames are not selected yet.
        s.selected_layer = None;
        s.selected_ranges.clear();
        let here = s.context.current_frame;
        if entered {
            s.frame_stack.push(here);
        }
        Ok(entered)
    });

    // -- a layer's colour transform and filters, at a frame ------------------
    //
    // Animate's "advanced layers": the layer carries the effect and applies it
    // to the artwork on it. Here that lands on the layer's artwork at the
    // keyframe covering `frame`, as the instance colour and the object filters.
    host_fn!(
        ctx,
        host,
        state,
        "setLayerColorTransform",
        |state, index: i32, frame: u32, json: String| {
            let v: serde_json::Value =
                serde_json::from_str(&json).map_err(|_| throw("expected a colour transform object"))?;
            let g = |k: &str, d: f64| v.get(k).and_then(|x| x.as_f64()).unwrap_or(d);
            let t = buzz_scene::ColorTransform {
                multiply: [
                    (g("colorRedPercent", 100.0) / 100.0) as f32,
                    (g("colorGreenPercent", 100.0) / 100.0) as f32,
                    (g("colorBluePercent", 100.0) / 100.0) as f32,
                    (g("colorAlphaPercent", 100.0) / 100.0) as f32,
                ],
                add: [
                    (g("colorRedAmount", 0.0) / 255.0) as f32,
                    (g("colorGreenAmount", 0.0) / 255.0) as f32,
                    (g("colorBlueAmount", 0.0) / 255.0) as f32,
                    (g("colorAlphaAmount", 0.0) / 255.0) as f32,
                ],
            };
            let mut s = state.borrow_mut();
            let id = layer_id(&s, index)?;
            // **A layer's effect goes on top of each instance's own colour**
            // (a tint set on the instance stays): the colour an instance had
            // before the first layer effect is remembered, so setting the effect
            // again replaces it instead of stacking.
            let start = s
                .scene()
                .layers()
                .get(id)
                .and_then(|l| l.frames.keyframe_start(frame))
                .unwrap_or(frame);
            let mut now: Vec<(ObjectId, buzz_scene::ColorTransform)> = Vec::new();
            if let Some(l) = s.scene().layers().get(id) {
                for o in l.frames.objects_at(frame).iter() {
                    if let ObjectKind::Instance(i) = &o.kind {
                        now.push((o.id, i.color));
                    }
                }
            }
            let mut composed = std::collections::HashMap::new();
            for (oid, current) in now {
                let base = *s.layer_fx_base.entry((oid.0, start)).or_insert(current);
                composed.insert(oid, base.compose(&t));
            }
            s.scene_mut().update_layer(id, |l| {
                if let Some(kf) = l.frames.keyframe_at_mut(frame) {
                    let objects = std::sync::Arc::make_mut(&mut kf.objects);
                    for o in objects.iter_mut() {
                        let oid = o.id;
                        if let ObjectKind::Instance(i) = &mut std::sync::Arc::make_mut(o).kind
                            && let Some(c) = composed.get(&oid)
                        {
                            i.color = *c;
                        }
                    }
                }
            });
            Ok(())
        }
    );
    host_fn!(
        ctx,
        host,
        state,
        "layerColorTransform",
        |state, index: i32, frame: u32| {
            let s = state.borrow();
            let id = layer_id(&s, index)?;
            let l = s.scene().layers().get(id).ok_or_else(|| throw("that layer is gone"))?;
            let t = l
                .frames
                .objects_at(frame)
                .iter()
                .find_map(|o| match &o.kind {
                    ObjectKind::Instance(i) => Some(i.color),
                    _ => None,
                })
                .unwrap_or_default();
            Ok(serde_json::json!({
                "colorRedPercent": t.multiply[0] as f64 * 100.0,
                "colorGreenPercent": t.multiply[1] as f64 * 100.0,
                "colorBluePercent": t.multiply[2] as f64 * 100.0,
                "colorAlphaPercent": t.multiply[3] as f64 * 100.0,
                "colorRedAmount": t.add[0] as f64 * 255.0,
                "colorGreenAmount": t.add[1] as f64 * 255.0,
                "colorBlueAmount": t.add[2] as f64 * 255.0,
                "colorAlphaAmount": t.add[3] as f64 * 255.0,
            })
            .to_string())
        }
    );
    host_fn!(
        ctx,
        host,
        state,
        "setLayerFilters",
        |state, index: i32, frame: u32, json: String| {
            let list: Vec<serde_json::Value> =
                serde_json::from_str(&json).map_err(|_| throw("expected an array of filters"))?;
            let mut filters = Vec::new();
            for f in &list {
                let name = f.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let num = |k: &str, d: f64| f.get(k).and_then(|x| x.as_f64()).unwrap_or(d);
                let quality = match f.get("quality").and_then(|x| x.as_str()).unwrap_or("low") {
                    "high" => buzz_fx::Quality::High,
                    "medium" => buzz_fx::Quality::Medium,
                    _ => buzz_fx::Quality::Low,
                };
                match name {
                    "blurFilter" => filters.push(buzz_fx::Filter::new(buzz_fx::FilterKind::Blur {
                        x: num("blurX", 5.0),
                        y: num("blurY", 5.0),
                        quality,
                    })),
                    "adjustColorFilter" => filters.push(buzz_fx::Filter::new(
                        buzz_fx::FilterKind::Adjust(buzz_fx::ColorAdjust {
                            brightness: num("brightness", 0.0),
                            contrast: num("contrast", 0.0),
                            saturation: num("saturation", 0.0),
                            hue: num("hue", 0.0),
                        }),
                    )),
                    other => {
                        return Err(throw(&format!(
                            "the filter {other:?} is not supported by setFiltersAtFrame yet \
                             (blurFilter and adjustColorFilter are)"
                        )));
                    }
                }
            }
            let mut s = state.borrow_mut();
            let id = layer_id(&s, index)?;
            s.scene_mut().update_layer(id, |l| {
                if let Some(kf) = l.frames.keyframe_at_mut(frame) {
                    let objects = std::sync::Arc::make_mut(&mut kf.objects);
                    for o in objects.iter_mut() {
                        std::sync::Arc::make_mut(o).filters = filters.clone();
                    }
                }
            });
            Ok(())
        }
    );
    host_fn!(ctx, host, state, "layerFilters", |state, index: i32, frame: u32| {
        let s = state.borrow();
        let id = layer_id(&s, index)?;
        let l = s.scene().layers().get(id).ok_or_else(|| throw("that layer is gone"))?;
        let mut out = Vec::new();
        if let Some(o) = l.frames.objects_at(frame).first() {
            for f in &o.filters {
                if let buzz_fx::FilterKind::Blur { x, y, .. } = &f.kind {
                    out.push(serde_json::json!({"name": "blurFilter", "blurX": x, "blurY": y}));
                } else {
                    out.push(serde_json::json!({"name": f.label()}));
                }
            }
        }
        Ok(serde_json::Value::Array(out).to_string())
    });

    // -- FLfile ---------------------------------------------------------------
    host_fn!(ctx, host, state, "fileRead", |state, uri: String| {
        let s = state.borrow();
        let r = resolve(&s, &uri).ok_or_else(|| not_allowed(&uri))?;
        for p in &r.read {
            if let Ok(text) = std::fs::read_to_string(p) {
                return Ok(Some(text));
            }
        }
        Ok(None::<String>)
    });
    host_fn!(ctx, host, state, "fileWrite", |state, uri: String, text: String, append: bool| {
        let s = state.borrow();
        let r = resolve(&s, &uri).ok_or_else(|| not_allowed(&uri))?;
        let path = r.write.ok_or_else(|| not_allowed(&uri))?;
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let result = if append {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(text.as_bytes()))
        } else {
            std::fs::write(&path, text)
        };
        Ok(result.is_ok())
    });
    host_fn!(ctx, host, state, "fileExists", |state, uri: String| {
        let s = state.borrow();
        Ok(resolve(&s, &uri).is_some_and(|r| r.read.iter().any(|p| p.exists())))
    });
    host_fn!(ctx, host, state, "fileRemove", |state, uri: String| {
        let s = state.borrow();
        let r = resolve(&s, &uri).ok_or_else(|| not_allowed(&uri))?;
        let path = r.write.ok_or_else(|| not_allowed(&uri))?;
        Ok(std::fs::remove_file(&path).is_ok() || std::fs::remove_dir_all(&path).is_ok())
    });
    host_fn!(ctx, host, state, "fileCreateFolder", |state, uri: String| {
        let s = state.borrow();
        let r = resolve(&s, &uri).ok_or_else(|| not_allowed(&uri))?;
        let path = r.write.ok_or_else(|| not_allowed(&uri))?;
        Ok(std::fs::create_dir_all(&path).is_ok())
    });
    host_fn!(ctx, host, state, "fileList", |state, uri: String, kind: String| {
        let s = state.borrow();
        let r = resolve(&s, &uri).ok_or_else(|| not_allowed(&uri))?;
        let mut names: Vec<String> = Vec::new();
        for dir in &r.read {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for e in entries.flatten() {
                let is_dir = e.path().is_dir();
                let want = match kind.as_str() {
                    "files" => !is_dir,
                    "directories" => is_dir,
                    _ => true,
                };
                if want {
                    let n = e.file_name().to_string_lossy().to_string();
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
        }
        names.sort();
        Ok(serde_json::to_string(&names).unwrap_or_else(|_| "[]".into()))
    });
    host_fn!(ctx, host, state, "uriToPath", |state, uri: String| {
        let _ = state;
        Ok(uri_path(&uri).to_string_lossy().replace('/', "\\"))
    });
    host_fn!(ctx, host, state, "pathToUri", |state, path: String| {
        let _ = state;
        let text = path.replace('\\', "/");
        let text = match text.as_bytes() {
            [drive, b':', ..] => format!("{}|{}", *drive as char, &text[2..]),
            _ => text,
        };
        Ok(format!("file:///{}", text.replace(' ', "%20")))
    });

    // -- asking the editor to save --------------------------------------------
    host_fn!(ctx, host, state, "requestSave", |state| {
        state.borrow_mut().save_requested = true;
        Ok(())
    });
    Ok(())
}
