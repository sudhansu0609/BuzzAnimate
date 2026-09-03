//! The primitives the JavaScript prelude is built on.
//!
//! Everything here is a plain function on a hidden `__host` object. The shape
//! of the API a user sees — `document.width`, `timeline.layers[0].name` — is
//! assembled from these by `prelude.js`.
//!
//! Keeping the split means the Rust side never deals with property accessors,
//! prototypes or `this`, and the part that has to match Animate is written in
//! JavaScript where it can be read against Animate's own documentation.
//!
//! # Indices
//!
//! Layers are addressed by index from the front of the stack, as Animate's
//! `timeline.layers` array is. Every function that takes one range-checks it
//! and throws, because a script that quietly does nothing is far harder to
//! debug than one that stops and says which index was wrong.

use std::cell::RefCell;
use std::rc::Rc;

use buzz_geom::{Affine, Rect, Shape as _};
use buzz_scene::{LayerId, LayerKind, ObjectId, ShapeData, SymbolKind};
use peniko::Color;
use rquickjs::{Ctx, Function, Object, Result as JsResult};

use crate::State;

/// Attach one host function, giving it a clone of the shared state.
///
/// A macro rather than a generic helper because rquickjs derives a function's
/// arity from the closure's *parameter list*: a helper taking a tuple compiles
/// to a one-argument JavaScript function, and there is no bound that says
/// otherwise. Expanding to a closure with the real parameters is what makes a
/// two-argument host function actually take two arguments.
macro_rules! host_fn {
    ($ctx:expr, $host:expr, $state:expr, $name:literal, |$s:ident $(, $arg:ident : $ty:ty)*| $body:block) => {{
        let shared = Rc::clone($state);
        let function = Function::new($ctx.clone(), move |$($arg: $ty),*| {
            let $s = &shared;
            // The annotation pins the error type. Without it a body whose only
            // statement is `Ok(...)` leaves the error generic, and rquickjs
            // cannot work out what kind of function it is being handed.
            //
            // The closure is load-bearing rather than redundant: it is what
            // gives each body its own `return`, so `return Err(throw(...))`
            // inside a host function fails that call rather than abandoning
            // the installation of every function after it.
            #[allow(
                clippy::redundant_closure_call,
                reason = "the closure scopes `return` to one host function"
            )]
            let result: JsResult<_> = (|| $body)();
            result
        })?;
        $host.set($name, function)?;
    }};
}

/// Bind every primitive onto a `__host` global.
pub(crate) fn install(ctx: &Ctx<'_>, state: &Rc<RefCell<State>>) -> JsResult<()> {
    let host = Object::new(ctx.clone())?;

    // -- output -------------------------------------------------------------
    host_fn!(ctx, host, state, "trace", |state, text: String| {
        state.borrow_mut().trace.push(text);
        Ok(())
    });

    // -- the questions a script asks of a person -----------------------------
    //
    // Nobody is watching a script run, so a modal question has nobody to
    // answer it. Recording it and answering with the default is what lets the
    // rest of the script run; the host shows the list when it is over.
    host_fn!(ctx, host, state, "alert", |state, text: String| {
        state.borrow_mut().alerts.push(text);
        Ok(())
    });
    host_fn!(
        ctx,
        host,
        state,
        "askedWith",
        |state, question: String, answer: String| {
            state
                .borrow_mut()
                .alerts
                .push(format!("{question} - answered {answer:?}"));
            Ok(())
        }
    );

    // -- the script shelf ---------------------------------------------------
    //
    // Animate hands scripts the Configuration folder as a `file:///` URI and
    // lets them pull each other in from it. A shelf of commands is written
    // against that: a shared settings file first, then the command itself.
    host_fn!(ctx, host, state, "configUri", |state| {
        Ok(state
            .borrow()
            .context
            .config_dir
            .as_deref()
            .map(path_to_uri)
            .unwrap_or_default())
    });
    host_fn!(ctx, host, state, "readScript", |state, uri: String| {
        let borrowed = state.borrow();
        let Some(root) = borrowed.context.config_dir.clone() else {
            return Err(throw(
                "this script asked to read another script, and no script folder is configured",
            ));
        };
        drop(borrowed);
        let path = uri_to_path(&uri)?;
        // **Confined to the folder.** A `..` in the URI, or an absolute path
        // somewhere else entirely, must not turn scripting into a way to read
        // the disk. Canonicalised first so the check cannot be walked around.
        let (real, root) = match (path.canonicalize(), root.canonicalize()) {
            (Ok(a), Ok(b)) => (a, b),
            _ => return Err(throw(&format!("no script at {uri}"))),
        };
        if !real.starts_with(&root) {
            return Err(throw(&format!(
                "{uri} is outside the script folder, which scripts may not read past"
            )));
        }
        std::fs::read_to_string(&real)
            .map_err(|e| throw(&format!("could not read {uri}: {e}")))
    });

    // -- document properties ------------------------------------------------
    host_fn!(ctx, host, state, "docWidth", |state| {
        Ok(state.borrow().scene.stage().size.width)
    });
    host_fn!(ctx, host, state, "docHeight", |state| {
        Ok(state.borrow().scene.stage().size.height)
    });
    host_fn!(ctx, host, state, "setDocSize", |state, w: f64, h: f64| {
        let mut s = state.borrow_mut();
        let size = &mut s.scene.stage_mut().size;
        if w > 0.0 {
            size.width = w;
        }
        if h > 0.0 {
            size.height = h;
        }
        Ok(())
    });
    host_fn!(ctx, host, state, "frameRate", |state| {
        Ok(state.borrow().scene.stage().frame_rate)
    });
    host_fn!(ctx, host, state, "setFrameRate", |state, rate: f64| {
        if rate > 0.0 {
            state.borrow_mut().scene.stage_mut().frame_rate = rate;
        }
        Ok(())
    });
    host_fn!(ctx, host, state, "backgroundColor", |state| {
        Ok(to_hex(state.borrow().scene.stage().background))
    });
    host_fn!(
        ctx,
        host,
        state,
        "setBackgroundColor",
        |state, hex: String| {
            let color = parse_color(&hex)?;
            state.borrow_mut().scene.stage_mut().background = color;
            Ok(())
        }
    );

    // -- layers -------------------------------------------------------------
    host_fn!(ctx, host, state, "layerCount", |state| {
        Ok(state.borrow().scene.layers().len() as i32)
    });
    host_fn!(ctx, host, state, "layerName", |state, index: i32| {
        let s = state.borrow();
        Ok(layer_at(&s, index)?.name.clone())
    });
    host_fn!(
        ctx,
        host,
        state,
        "setLayerName",
        |state, index: i32, name: String| {
            let id = layer_id(&state.borrow(), index)?;
            state.borrow_mut().scene.update_layer(id, |l| l.name = name);
            Ok(())
        }
    );
    host_fn!(ctx, host, state, "layerVisible", |state, index: i32| {
        let s = state.borrow();
        Ok(layer_at(&s, index)?.visible)
    });
    host_fn!(
        ctx,
        host,
        state,
        "setLayerVisible",
        |state, index: i32, on: bool| {
            let id = layer_id(&state.borrow(), index)?;
            state
                .borrow_mut()
                .scene
                .update_layer(id, |l| l.visible = on);
            Ok(())
        }
    );
    host_fn!(ctx, host, state, "layerLocked", |state, index: i32| {
        let s = state.borrow();
        Ok(layer_at(&s, index)?.locked)
    });
    host_fn!(
        ctx,
        host,
        state,
        "setLayerLocked",
        |state, index: i32, on: bool| {
            let id = layer_id(&state.borrow(), index)?;
            state.borrow_mut().scene.update_layer(id, |l| l.locked = on);
            Ok(())
        }
    );
    host_fn!(ctx, host, state, "layerDepth", |state, index: i32| {
        let s = state.borrow();
        Ok(layer_at(&s, index)?.depth)
    });
    host_fn!(
        ctx,
        host,
        state,
        "setLayerDepth",
        |state, index: i32, depth: f64| {
            if !depth.is_finite() {
                return Err(throw("layer depth must be a finite number"));
            }
            let id = layer_id(&state.borrow(), index)?;
            state
                .borrow_mut()
                .scene
                .update_layer(id, |l| l.depth = depth);
            Ok(())
        }
    );
    host_fn!(ctx, host, state, "addNewLayer", |state, name: String| {
        let mut s = state.borrow_mut();
        let name = if name.is_empty() {
            format!("Layer_{}", s.scene.layers().len() + 1)
        } else {
            name
        };
        s.scene.add_layer(name, LayerKind::Normal);
        Ok(())
    });
    host_fn!(ctx, host, state, "deleteLayer", |state, index: i32| {
        let id = layer_id(&state.borrow(), index)?;
        let mut s = state.borrow_mut();
        // Animate refuses to remove the last layer: a document with none has
        // nowhere to draw.
        if s.scene.layers().len() <= 1 {
            return Err(throw("a document must keep at least one layer"));
        }
        s.scene.remove_layer(id);
        Ok(())
    });

    // -- frames -------------------------------------------------------------
    host_fn!(ctx, host, state, "frameCount", |state| {
        Ok(state.borrow().scene.frame_count() as i32)
    });
    host_fn!(ctx, host, state, "currentFrame", |state| {
        Ok(state.borrow().context.current_frame as i32)
    });
    host_fn!(ctx, host, state, "setCurrentFrame", |state, frame: i32| {
        if frame < 0 {
            return Err(throw("a frame number cannot be negative"));
        }
        state.borrow_mut().context.current_frame = frame as u32;
        Ok(())
    });
    host_fn!(ctx, host, state, "insertFrames", |state, count: i32| {
        let (id, frame) = target(&state.borrow())?;
        let mut s = state.borrow_mut();
        for _ in 0..count.max(0) {
            s.scene.update_layer(id, |l| {
                l.frames.insert_frame(frame);
            });
        }
        Ok(())
    });
    host_fn!(ctx, host, state, "insertKeyframe", |state| {
        let (id, frame) = target(&state.borrow())?;
        state.borrow_mut().scene.update_layer(id, |l| {
            l.frames.insert_keyframe(frame);
        });
        Ok(())
    });
    host_fn!(ctx, host, state, "insertBlankKeyframe", |state| {
        let (id, frame) = target(&state.borrow())?;
        state.borrow_mut().scene.update_layer(id, |l| {
            l.frames.insert_blank_keyframe(frame);
        });
        Ok(())
    });

    // -- drawing ------------------------------------------------------------
    // Two functions rather than one with a `kind` argument: rquickjs binds
    // native functions up to seven parameters, and a shape needs four for its
    // rectangle plus three for its paint. That is exactly seven.
    host_fn!(
        ctx,
        host,
        state,
        "addRectangle",
        |state, l: f64, t: f64, r: f64, b: f64, fill: String, stroke: String, width: f64| {
            add_shape(state, false, l, t, r, b, &fill, &stroke, width)
        }
    );
    host_fn!(
        ctx,
        host,
        state,
        "addOval",
        |state, l: f64, t: f64, r: f64, b: f64, fill: String, stroke: String, width: f64| {
            add_shape(state, true, l, t, r, b, &fill, &stroke, width)
        }
    );

    // -- animation ----------------------------------------------------------
    //
    // Everything below drives the *document model*, not the editor's commands.
    //
    // A script holds a working copy of the scene and reads back what it wrote;
    // a command needs the editor's selection, style and undo stack, and running
    // one in the middle of a script would be writing to a different document
    // from the one the next line reads. So the automation a script gets is the
    // model's own vocabulary — tweens, camera keys, modifiers, performances —
    // rather than a way to press the menu items.

    host_fn!(
        ctx,
        host,
        state,
        "setTween",
        |state, layer: i32, frame: u32, kind: String| {
            let mut s = state.borrow_mut();
            let id = layer_id(&s, layer)?;
            let tween = match kind.as_str() {
                "motion" => buzz_scene::Tween::motion(),
                "classic" => buzz_scene::Tween::classic(),
                "shape" => buzz_scene::Tween::shape(),
                "none" => buzz_scene::Tween::default(),
                other => {
                    return Err(throw(&format!(
                        "unknown tween {other:?}; expected motion, classic, shape or none"
                    )));
                }
            };
            let mut set = false;
            s.scene.update_layer(id, |l| {
                set = l.frames.set_tween(frame, tween);
            });
            if !set {
                return Err(throw(&format!(
                    "frame {frame} of layer {layer} has no keyframe to tween from"
                )));
            }
            Ok(())
        }
    );

    host_fn!(
        ctx,
        host,
        state,
        "setEase",
        |state, layer: i32, frame: u32, strength: f64| {
            let mut s = state.borrow_mut();
            let id = layer_id(&s, layer)?;
            let mut set = false;
            s.scene.update_layer(id, |l| {
                let mut tween = l.frames.tween_at(frame);
                if !tween.is_active() {
                    return;
                }
                // Animate's own number: -100 slows in, +100 slows out.
                tween.easing = buzz_scene::Easing::Strength(strength.clamp(-100.0, 100.0));
                set = l.frames.set_tween(frame, tween);
            });
            if !set {
                return Err(throw(&format!(
                    "frame {frame} of layer {layer} has no tween to ease"
                )));
            }
            Ok(())
        }
    );

    // -- the camera ---------------------------------------------------------
    host_fn!(ctx, host, state, "setCameraEnabled", |state, on: bool| {
        state.borrow_mut().scene.camera_mut().enabled = on;
        Ok(())
    });

    host_fn!(
        ctx,
        host,
        state,
        "setCameraKey",
        |state, frame: u32, x: f64, y: f64, zoom: f64, rotation: f64| {
            let mut s = state.borrow_mut();
            let camera = s.scene.camera_mut();
            camera.enabled = true;
            let mut key = buzz_scene::CameraKey::new(frame, buzz_geom::Point::new(x, y));
            key.zoom = if zoom > 0.0 { zoom } else { 1.0 };
            key.rotation = rotation.to_radians();
            camera.set_key(key.clamped());
            Ok(())
        }
    );

    host_fn!(ctx, host, state, "removeCameraKey", |state, frame: u32| {
        Ok(state.borrow_mut().scene.camera_mut().remove_key(frame))
    });

    host_fn!(
        ctx,
        host,
        state,
        "setFocusKey",
        |state, frame: u32, depth: f64, aperture: f64| {
            let mut s = state.borrow_mut();
            s.scene.camera_mut().set_focus_key(buzz_scene::FocusKey {
                frame,
                focus_depth: depth,
                aperture: aperture.max(0.0),
            });
            Ok(())
        }
    );

    host_fn!(
        ctx,
        host,
        state,
        "setShutter",
        |state, shutter: f64, samples: i32| {
            let mut s = state.borrow_mut();
            let camera = s.scene.camera_mut();
            camera.shutter = shutter.max(0.0);
            if samples > 0 {
                camera.blur_samples = samples as u32;
            }
            Ok(())
        }
    );

    // -- live modifiers -----------------------------------------------------
    host_fn!(
        ctx,
        host,
        state,
        "addWiggle",
        |state, amplitude: f64, frequency: f64| {
            let modifier = buzz_scene::Modifier::Wiggle {
                amplitude,
                frequency,
            };
            add_modifier(state, modifier)
        }
    );

    host_fn!(
        ctx,
        host,
        state,
        "addSpring",
        |state, stiffness: f64, damping: f64, coupling: f64| {
            let modifier = buzz_scene::Modifier::Spring {
                root: 0,
                stiffness,
                damping,
                coupling,
            };
            add_modifier(state, modifier)
        }
    );

    host_fn!(ctx, host, state, "clearModifiers", |state| {
        let mut s = state.borrow_mut();
        let ids = s.context.selection.clone();
        if ids.is_empty() {
            return Err(throw("nothing is selected to clear modifiers from"));
        }
        for id in ids {
            s.scene.update_object_across(0, u32::MAX, id, |o| {
                o.modifiers.clear();
            });
        }
        Ok(())
    });

    // -- text ---------------------------------------------------------------
    host_fn!(
        ctx,
        host,
        state,
        "addText",
        |state, x: f64, y: f64, content: String, size: f64, font: String| {
            let mut s = state.borrow_mut();
            let layer = s
                .context
                .active_layer
                .or_else(|| s.scene.layers().iter().next().map(|l| l.id))
                .ok_or_else(|| throw("the document has no layer to put text on"))?;
            let frame = s.context.current_frame;
            let family = (!font.is_empty()).then(|| font.clone());
            let size = if size > 0.0 { size } else { 48.0 };
            let path = buzz_text::outline(&content, size, family.as_deref())
                .ok_or_else(|| throw("there is no font available to draw text with"))?;
            let colour = Color::BLACK;
            let id = s
                .scene
                .add_shape_at(layer, frame, ShapeData::filled(path, colour))
                .ok_or_else(|| throw("could not put text on that frame"))?;
            s.scene.update_object_at(frame, id, |o| {
                o.transform = Affine::translate((x, y));
                o.text = Some(buzz_scene::TextData::new(&content, size, family.clone()));
            });
            Ok(id.0)
        }
    );

    // -- the performance ----------------------------------------------------
    //
    // The two calls that make a script worth writing: a walk on a character,
    // and a whole staged scene from a line of prose. Both are ordinary layers,
    // shapes and keyframes when they land, so a script can go on to edit what
    // it just produced.

    host_fn!(
        ctx,
        host,
        state,
        "perform",
        |state, object: u64, action: String, from: u32, to: u32| {
            let mut s = state.borrow_mut();
            let action = match action.to_ascii_lowercase().as_str() {
                "walk" => buzz_act::perform::Action::Walk,
                "run" => buzz_act::perform::Action::Run,
                "talk" => buzz_act::perform::Action::Talk,
                "idle" => buzz_act::perform::Action::Idle,
                other => {
                    return Err(throw(&format!(
                        "unknown action {other:?}; expected walk, run, talk or idle"
                    )));
                }
            };
            if to <= from {
                return Err(throw("a performance needs at least one frame"));
            }
            let performance = buzz_act::perform::Performance::new(action, from..to);
            buzz_act::perform::apply(&mut s.scene, ObjectId(object), &performance)
                .map(|report| report.keyframes as i32)
                .map_err(|e| throw(&format!("{e}")))
        }
    );

    host_fn!(ctx, host, state, "direct", |state, story: String| {
        let mut s = state.borrow_mut();
        buzz_act::direct(&mut s.scene, &story)
            .map(|scene| scene.frames as i32)
            .map_err(|e| throw(&format!("{e}")))
    });

    // -- selection ----------------------------------------------------------
    host_fn!(ctx, host, state, "selectionCount", |state| {
        Ok(state.borrow().context.selection.len() as i32)
    });
    host_fn!(ctx, host, state, "selectAll", |state| {
        let mut s = state.borrow_mut();
        let frame = s.context.current_frame;
        let ids: Vec<ObjectId> = s
            .scene
            .layers()
            .selectable()
            .flat_map(|l| l.objects_at(frame).iter())
            .filter(|o| o.visible && !o.locked)
            .map(|o| o.id)
            .collect();
        s.context.selection = ids;
        Ok(())
    });
    host_fn!(ctx, host, state, "selectNone", |state| {
        state.borrow_mut().context.selection.clear();
        Ok(())
    });
    host_fn!(
        ctx,
        host,
        state,
        "moveSelectionBy",
        |state, dx: f64, dy: f64| {
            if !dx.is_finite() || !dy.is_finite() {
                return Err(throw("a move needs finite distances"));
            }
            let ids = state.borrow().context.selection.clone();
            let mut s = state.borrow_mut();
            for id in ids {
                s.scene.update_object(id, |o| {
                    o.transform = Affine::translate((dx, dy)) * o.transform;
                });
            }
            Ok(())
        }
    );
    host_fn!(ctx, host, state, "deleteSelection", |state| {
        let ids = state.borrow().context.selection.clone();
        let mut s = state.borrow_mut();
        for id in ids {
            s.scene.remove_object(id);
        }
        s.context.selection.clear();
        Ok(())
    });

    // -- library and symbols ------------------------------------------------
    host_fn!(ctx, host, state, "libraryItemCount", |state| {
        Ok(state.borrow().scene.library().len() as i32)
    });
    host_fn!(ctx, host, state, "libraryItemName", |state, index: i32| {
        let s = state.borrow();
        let names: Vec<String> = s.scene.library().iter().map(|x| x.name.clone()).collect();
        names
            .get(index.max(0) as usize)
            .cloned()
            .ok_or_else(|| throw(&format!("there is no library item {index}")))
    });
    host_fn!(
        ctx,
        host,
        state,
        "convertToSymbol",
        |state, kind: String, name: String| {
            let kind = match kind.to_ascii_lowercase().as_str() {
                "graphic" => SymbolKind::Graphic,
                "button" => SymbolKind::Button,
                "movie clip" | "movieclip" | "movie" => SymbolKind::MovieClip,
                other => {
                    return Err(throw(&format!(
                        "unknown symbol type {other:?}; expected \"graphic\", \
                         \"movie clip\" or \"button\""
                    )));
                }
            };

            let ids = state.borrow().context.selection.clone();
            if ids.is_empty() {
                return Err(throw("select something before converting it to a symbol"));
            }

            let (layer, frame) = target(&state.borrow())?;
            let mut s = state.borrow_mut();

            let name = if name.is_empty() {
                "Symbol".to_string()
            } else {
                name
            };
            let symbol = s.scene.add_symbol(name, kind, None);
            let Some(inner) = s
                .scene
                .library()
                .get(symbol)
                .and_then(|x| x.layers.iter().next())
                .map(|l| l.id)
            else {
                return Err(throw("the new symbol has no layer to fill"));
            };

            let mut lifted = Vec::new();
            for id in &ids {
                if let Some(object) = s.scene.remove_object(*id) {
                    lifted.push(object);
                }
            }
            s.scene.library_mut().update(symbol, |sym| {
                sym.layers.update(inner, |l| {
                    l.frames.set_objects(0, lifted);
                });
            });

            let placed = s
                .scene
                .add_instance_at(layer, frame, symbol, Affine::IDENTITY);
            s.context.selection = placed.into_iter().collect();
            Ok(())
        }
    );

    ctx.globals().set("__host", host)?;
    Ok(())
}

/// Add a rectangle or an oval to the layer the editor is working on.
#[allow(clippy::too_many_arguments, reason = "mirrors the script-facing call")]
fn add_shape(
    state: &Rc<RefCell<State>>,
    oval: bool,
    l: f64,
    t: f64,
    r: f64,
    b: f64,
    fill: &str,
    stroke: &str,
    width: f64,
) -> JsResult<()> {
    for value in [l, t, r, b] {
        if !value.is_finite() {
            return Err(throw("a shape needs finite coordinates"));
        }
    }
    let rect = Rect::new(l.min(r), t.min(b), l.max(r), t.max(b));
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return Err(throw("a shape needs a non-zero width and height"));
    }

    let path = if oval {
        kurbo::Ellipse::from_rect(rect).to_path(1e-3)
    } else {
        rect.to_path(1e-9)
    };

    let mut shape = ShapeData {
        path,
        fill: None,
        stroke: None,
        blend: buzz_scene::PaintBlend::Normal,
    };
    if !fill.is_empty() {
        shape.fill = Some(buzz_scene::FillSpec::solid(parse_color(fill)?));
    }
    if !stroke.is_empty() {
        shape.stroke = Some(buzz_scene::StrokeSpec::new(
            parse_color(stroke)?,
            width.max(0.0),
        ));
    }
    if shape.fill.is_none() && shape.stroke.is_none() {
        return Err(throw(
            "a shape with neither a fill nor a stroke would be invisible",
        ));
    }

    let (id, frame) = target(&state.borrow())?;
    let mut s = state.borrow_mut();
    match s.scene.add_shape_at(id, frame, shape) {
        Some(_) => Ok(()),
        None => Err(throw("could not draw on that layer; it may be locked")),
    }
}

/// A JavaScript exception carrying `message`.
/// A path as JSFL writes it: `file:///C|/Users/...`.
///
/// Animate's own spelling, drive letter and all — the pipe rather than a colon
/// is a genuine quirk of the format and scripts concatenate onto it, so it has
/// to come back out the same way it went in.
fn path_to_uri(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = match text.as_bytes() {
        [drive, b':', ..] => format!("{}|{}", *drive as char, &text[2..]),
        _ => text.to_string(),
    };
    let text = text.trim_end_matches('/').to_string();
    format!("file:///{text}/")
}

/// The reverse, accepting the plain path a script might also pass.
fn uri_to_path(uri: &str) -> JsResult<std::path::PathBuf> {
    let text = uri.trim();
    let text = text
        .strip_prefix("file:///")
        .or_else(|| text.strip_prefix("file://"))
        .unwrap_or(text);
    // `C|/...` back to `C:/...`, and percent-escaped spaces back to spaces —
    // both of which turn up in a real Configuration path.
    let text = text.replace("%20", " ");
    let text = match text.as_bytes() {
        [drive, b'|', ..] => format!("{}:{}", *drive as char, &text[2..]),
        _ => text.to_string(),
    };
    if text.is_empty() {
        return Err(throw("expected the path of a script"));
    }
    Ok(std::path::PathBuf::from(text))
}

/// Put a live modifier on everything selected.
///
/// Across the whole film rather than on one frame, because a modifier is a
/// property of the object and not of a moment — the same reason the editor's
/// own Add Wiggle writes it that way.
fn add_modifier(
    state: &Rc<RefCell<State>>,
    modifier: buzz_scene::Modifier,
) -> JsResult<()> {
    let mut s = state.borrow_mut();
    let ids = s.context.selection.clone();
    if ids.is_empty() {
        return Err(throw("nothing is selected to put a modifier on"));
    }
    for id in ids {
        s.scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(modifier);
        });
    }
    Ok(())
}

fn throw(message: &str) -> rquickjs::Error {
    // The engine turns this into a thrown `Error` at the call site, which is
    // what lets a script `try`/`catch` a bad index.
    rquickjs::Error::new_from_js_message("host", "error", message)
}

/// The layer at `index`, counted from the front of the stack.
fn layer_at(state: &State, index: i32) -> JsResult<&buzz_scene::Layer> {
    if index < 0 {
        return Err(throw("a layer index cannot be negative"));
    }
    state
        .scene
        .layers()
        .iter()
        .nth(index as usize)
        .map(|l| l.as_ref())
        .ok_or_else(|| {
            throw(&format!(
                "there is no layer {index}; the document has {}",
                state.scene.layers().len()
            ))
        })
}

fn layer_id(state: &State, index: i32) -> JsResult<LayerId> {
    layer_at(state, index).map(|l| l.id)
}

/// Where a drawing or frame operation lands: the active layer, or the front
/// one if the editor has not nominated a layer.
fn target(state: &State) -> JsResult<(LayerId, u32)> {
    let layer = state
        .context
        .active_layer
        .filter(|id| state.scene.layers().get(*id).is_some())
        .or_else(|| state.scene.layers().iter().next().map(|l| l.id))
        .ok_or_else(|| throw("the document has no layer to work on"))?;
    Ok((layer, state.context.current_frame))
}

fn to_hex(color: Color) -> String {
    let [r, g, b, _] = color.to_rgba8().to_u8_array();
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Parse `#RRGGBB` or `#RRGGBBAA`, as JSFL colour strings are written.
fn parse_color(text: &str) -> JsResult<Color> {
    let hex = text.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();

    let parsed = match hex.len() {
        6 => byte(0)
            .zip(byte(2))
            .zip(byte(4))
            .map(|((r, g), b)| Color::from_rgba8(r, g, b, 255)),
        8 => byte(0)
            .zip(byte(2))
            .zip(byte(4))
            .zip(byte(6))
            .map(|(((r, g), b), a)| Color::from_rgba8(r, g, b, a)),
        _ => None,
    };
    parsed.ok_or_else(|| {
        throw(&format!(
            "{text:?} is not a colour; expected \"#RRGGBB\" or \"#RRGGBBAA\""
        ))
    })
}
