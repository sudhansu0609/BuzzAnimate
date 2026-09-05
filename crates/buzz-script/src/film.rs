//! **The calls that make a film**, rather than the calls that edit a drawing.
//!
//! [`crate::host`] is JSFL: rectangles, layers, keyframes, the vocabulary a
//! shelf of Animate commands is written in. It stops exactly where Animate's
//! own scripting stops, and for a program whose point is that *a brief at
//! midnight is an mp4 at breakfast* that turned out to be half an API.
//!
//! What was missing was not more drawing. It was everything either side of the
//! drawing:
//!
//! | Missing | Consequence |
//! |---|---|
//! | Scene management | a script could build one shot and never a second |
//! | Staging and scenery | a set had to come from a dialog somebody clicked |
//! | Casting with a face | nothing could blink, and nothing could talk |
//! | Rigging | the parenting column was unreachable |
//! | Dialogue | lip sync was a dialog, once per character, per shot |
//! | The asset library | a cast could not survive the document it was made in |
//!
//! Every one of those already existed and was reachable only by hand. This is
//! the wiring, and it is deliberately thin: each call below is a few lines over
//! a function in `buzz-act`, `buzz-scene` or `buzz-doc`, so what a script
//! produces is exactly what the menu item produces.
//!
//! # Two conventions worth knowing before reading further
//!
//! **Layers, objects and symbols are addressed by id, not by index.** JSFL
//! numbers layers from the top of the stack, which is fine for a person looking
//! at a timeline and wrong for a script that is *building* one: every layer
//! added renumbers the layers already there, so an index captured three lines
//! ago points somewhere else. Ids do not move.
//!
//! **The wide calls take and return JSON.** rquickjs binds a native function of
//! at most seven parameters, and staging a scene has more knobs than that. A
//! JSON object is also how a call answers with a whole rig — six ids and two
//! points — instead of forcing six calls to ask for them one at a time.
//!
//! # What is still not here, and why
//!
//! **Nothing opens a file and nothing writes one.** That was the right call
//! when scripting was designed and it stays right: the interpreter that must be
//! least able to break the program is the wrong place to put the encoder. Where
//! a script genuinely needs something off the disk — a dialogue track — the
//! *host* opens it before the run and hands the decoded clip over by index. The
//! asset library is the single exception, and it is confined to its own
//! directory in exactly the way `fl.runScript` is confined to the
//! Configuration folder.

use std::cell::RefCell;
use std::rc::Rc;

use buzz_geom::{Affine, Point};
use buzz_scene::{LayerId, LayerKind, Modifier, ObjectId, Scene, SymbolId, SymbolKind};
use rquickjs::{Ctx, Function, Object as JsObject, Result as JsResult};
use serde_json::{Value, json};

use crate::State;
use crate::host::{host_fn, parse_color, throw};

/// Install the film-making calls onto the `__host` object.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    host: &JsObject<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    scenes(ctx, host, state)?;
    sets(ctx, host, state)?;
    cast(ctx, host, state)?;
    rigging(ctx, host, state)?;
    dialogue(ctx, host, state)?;
    library(ctx, host, state)?;
    camera(ctx, host, state)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Scenes
// ---------------------------------------------------------------------------

/// **The film, not the shot.**
///
/// The one addition without which none of the others compose: a script that can
/// stage, cast and perform a scene but cannot make a second one can automate a
/// shot and never a film.
fn scenes<'js>(
    ctx: &Ctx<'js>,
    host: &JsObject<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    host_fn!(ctx, host, state, "sceneCount", |state| {
        Ok(state.borrow().scenes.len() as i32)
    });
    host_fn!(ctx, host, state, "sceneIndex", |state| {
        Ok(state.borrow().current as i32)
    });
    host_fn!(ctx, host, state, "setSceneIndex", |state, index: i32| {
        let mut s = state.borrow_mut();
        let count = s.scenes.len();
        if index < 0 || index as usize >= count {
            return Err(throw(&format!(
                "there is no scene {index}; the film has {count}"
            )));
        }
        s.current = index as usize;
        // The editor's own idea of where it is belongs to the scene it was in.
        s.context.selection.clear();
        s.context.active_layer = None;
        s.context.current_frame = 0;
        Ok(())
    });
    host_fn!(ctx, host, state, "addScene", |state, name: String| {
        let mut s = state.borrow_mut();
        // **The new scene inherits the stage, not the artwork.** Size and frame
        // rate are properties of the *film* -- a second shot at a different
        // frame rate is a mistake every time -- and everything else is the shot
        // itself, which is what the script is about to build.
        let stage = s.scene().stage().clone();
        let mut scene = Scene::default();
        *scene.stage_mut() = stage;
        s.scenes.push(scene);
        let index = s.scenes.len() - 1;
        s.names.push(if name.is_empty() {
            format!("Scene {}", index + 1)
        } else {
            name
        });
        s.current = index;
        s.context.selection.clear();
        s.context.active_layer = None;
        s.context.current_frame = 0;
        Ok(index as i32)
    });
    host_fn!(ctx, host, state, "sceneName", |state, index: i32| {
        let s = state.borrow();
        s.names
            .get(index.max(0) as usize)
            .cloned()
            .ok_or_else(|| throw(&format!("there is no scene {index}")))
    });
    host_fn!(
        ctx,
        host,
        state,
        "setSceneName",
        |state, index: i32, name: String| {
            let mut s = state.borrow_mut();
            let Some(slot) = s.names.get_mut(index.max(0) as usize) else {
                return Err(throw(&format!("there is no scene {index}")));
            };
            *slot = name;
            Ok(())
        }
    );
    // How long this scene runs. Every layer is stretched to reach it, because
    // a layer that ends early is a layer that vanishes mid-shot.
    host_fn!(ctx, host, state, "setSceneFrames", |state, frames: u32| {
        let mut s = state.borrow_mut();
        let frames = frames.max(1);
        let last = frames - 1;

        // **The camera first, because it can hold a shot open on its own.**
        //
        // `Scene::frame_count` is the longest layer *or* the last camera key,
        // whichever is further out -- which is right, since a shot must not end
        // before its focus pull arrives. It also means trimming the layers and
        // stopping there leaves the scene exactly as long as it was, and the
        // call silently does nothing. That is what a directed shot clamped to
        // five seconds ran into: every layer came back to 120 and the shot
        // stayed 151, because the director had keyed the camera at 150.
        let camera = s.scene_mut().camera_mut();
        let beyond: Vec<u32> = camera
            .keys()
            .iter()
            .map(|k| k.frame)
            .filter(|f| *f > last)
            .collect();
        for frame in beyond {
            camera.remove_key(frame);
        }
        let beyond: Vec<u32> = camera
            .focus_keys()
            .iter()
            .map(|k| k.frame)
            .filter(|f| *f > last)
            .collect();
        for frame in beyond {
            camera.remove_focus_key(frame);
        }

        s.scene_mut().set_frame_count(frames);

        // And every layer reaches the end, so nothing vanishes mid-shot.
        let layers: Vec<LayerId> = s.scene().layers().iter().map(|l| l.id).collect();
        for layer in layers {
            s.scene_mut().update_layer(layer, |l| {
                if l.frames.length() <= last {
                    l.frames.insert_frame(last);
                }
            });
        }
        Ok(s.scene().frame_count() as i32)
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Sets
// ---------------------------------------------------------------------------

fn sets<'js>(
    ctx: &Ctx<'js>,
    host: &JsObject<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    // **Set the scene**: ground, backdrop, a light rig, and optionally cloud
    // and water. The dialog's own call, with the dialog taken off it.
    host_fn!(ctx, host, state, "setTheScene", |state, options: String| {
        let options = parse_json(&options)?;
        let mut recipe = buzz_act::staging::SceneRecipe::default();
        if let Some(setting) = options.get("setting").and_then(Value::as_str) {
            recipe.setting = setting_named(setting)?;
        }
        recipe.cast = options.get("cast").and_then(Value::as_u64).unwrap_or(0) as usize;
        if let Some(v) = number(&options, "horizon") {
            recipe.horizon = v;
        }
        if let Some(v) = number(&options, "figureScale") {
            recipe.figure_scale = v;
        }
        if let Some(v) = options.get("lit").and_then(Value::as_bool) {
            recipe.lit = v;
        }
        if let Some(v) = options.get("frames").and_then(Value::as_u64) {
            recipe.frames = v as u32;
        }
        recipe.clouds = options.get("clouds").and_then(Value::as_bool).unwrap_or(false);
        recipe.water = options.get("water").and_then(Value::as_bool).unwrap_or(false);

        let mut s = state.borrow_mut();
        let staged = buzz_act::staging::build(s.scene_mut(), &recipe);
        let stage = s.scene().stage().stage_rect();
        let horizon_y = stage.y0 + stage.height() * recipe.horizon.clamp(0.15, 0.95);

        Ok(json!({
            "horizonY": horizon_y,
            "backdrop": staged.backdrop.map(|l| l.0).unwrap_or(0),
            "clouds": staged.clouds.map(|l| l.0).unwrap_or(0),
            "ground": staged.ground.map(|l| l.0).unwrap_or(0),
            "water": staged.water.map(|l| l.0).unwrap_or(0),
            "cast": staged.cast.iter().map(|(l, o)| json!({"layer": l.0, "object": o.0}))
                .collect::<Vec<_>>(),
            "message": staged.message,
        })
        .to_string())
    });

    // **Scenery from the effect brushes** -- a treeline, a skyline, a village,
    // grass -- laid at the horizon the set was built to.
    host_fn!(
        ctx,
        host,
        state,
        "layScenery",
        |state, kind: String, horizon_y: f64, backdrop: u64| {
            let what = scenery_named(&kind)?;
            let mut s = state.borrow_mut();
            let sky = (backdrop != 0).then_some(LayerId(backdrop));
            let report = buzz_act::scenery::lay(s.scene_mut(), what, horizon_y, sky);
            Ok(json!({
                "pieces": report.pieces,
                "layers": report.layers.iter().map(|l| l.0).collect::<Vec<_>>(),
            })
            .to_string())
        }
    );

    // **Weather.** Rain, snow, stars -- laid across the whole stage as one
    // effect-brush stroke, which is what `Set the Scene` does for a storm.
    host_fn!(ctx, host, state, "layWeather", |state, kind: String| {
        let effect = effect_named(&kind)?;
        let mut s = state.borrow_mut();
        let report = buzz_act::scenery::lay_weather(s.scene_mut(), effect);
        Ok(report.pieces as i32)
    });

    // A light. `json` carries whatever the kind needs: a sun's azimuth and
    // elevation, a lamp's position and reach, everything's colour and
    // intensity.
    host_fn!(ctx, host, state, "addLight", |state, kind: String, options: String| {
        let options = parse_json(&options)?;
        let mut s = state.borrow_mut();
        let stage = s.scene().stage().stage_rect();
        let light_kind = match kind.to_ascii_lowercase().as_str() {
            "sun" => buzz_scene::LightKind::Sun {
                azimuth: number(&options, "azimuth").unwrap_or(-0.6),
                elevation: number(&options, "elevation").unwrap_or(0.9),
            },
            "sky" => buzz_scene::LightKind::Sky {
                horizon: colour(&options, "horizon")?
                    .unwrap_or(peniko::Color::from_rgb8(0x6E, 0x86, 0xA8)),
            },
            "lamp" => buzz_scene::LightKind::Lamp {
                position: Point::new(
                    number(&options, "x").unwrap_or(stage.center().x),
                    number(&options, "y").unwrap_or(stage.center().y),
                ),
                height: number(&options, "height").unwrap_or(160.0),
                radius: number(&options, "reach")
                    .unwrap_or((stage.width().min(stage.height()) * 0.5).max(40.0)),
            },
            other => {
                return Err(throw(&format!(
                    "unknown light {other:?}; expected sun, sky or lamp"
                )));
            }
        };
        let id = s.scene_mut().add_light(light_kind);
        let tint = colour(&options, "color")?;
        let intensity = number(&options, "intensity");
        if let Some(light) = s.scene_mut().lights_mut().get_mut(id) {
            if let Some(tint) = tint {
                light.color = tint;
            }
            if let Some(intensity) = intensity {
                light.intensity = intensity as f32;
            }
        }
        Ok(id.0)
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Cast
// ---------------------------------------------------------------------------

fn cast<'js>(
    ctx: &Ctx<'js>,
    host: &JsObject<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    // **Cast a character**: a bone-rigged body, a face parented to it, a
    // blink, and a breath. See [`buzz_act::puppet`].
    host_fn!(ctx, host, state, "addCharacter", |state, options: String| {
        let options = parse_json(&options)?;
        let mut s = state.borrow_mut();
        let stage = s.scene().stage().stage_rect();

        let mut figure = buzz_act::figure::FigureSpec::default();
        if let Some(v) = number(&options, "height") {
            figure.height = v;
        }
        if let Some(v) = number(&options, "headRatio") {
            figure.head_ratio = v;
        }
        if let Some(v) = number(&options, "facing") {
            figure.facing = v;
        }
        if let Some(c) = colour(&options, "skin")? {
            figure.palette.skin = c;
        }
        if let Some(c) = colour(&options, "shirt")? {
            figure.palette.shirt = c;
        }
        if let Some(c) = colour(&options, "trousers")? {
            figure.palette.trousers = c;
        }

        let spec = buzz_act::puppet::PuppetSpec {
            name: options
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Person")
                .to_string(),
            figure,
            at: Point::new(
                number(&options, "x").unwrap_or(stage.center().x),
                number(&options, "y").unwrap_or(stage.y1 - stage.height() * 0.08),
            ),
            blink_rate: number(&options, "blink").unwrap_or(12.0),
            breathe_rate: number(&options, "breathe").unwrap_or(14.0),
            eyes: colour(&options, "eyes")?
                .unwrap_or(peniko::Color::from_rgb8(0x2A, 0x1C, 0x12)),
            frames: options
                .get("frames")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| s.scene().frame_count().max(1) as u64) as u32,
        };
        let puppet = buzz_act::puppet::build(s.scene_mut(), &spec);
        Ok(puppet_json(&puppet).to_string())
    });

    // **Direct a whole shot, and be told who it cast and when they speak.**
    //
    // `document.direct` has always staged, cast, blocked and framed a scene
    // from prose, and then handed back only how long it came out -- so a script
    // that wanted to lip-sync the people the director had just cast had no way
    // to find out who they were, or which frames the director had planned them
    // talking over. Both were computed and thrown away.
    //
    // That is the gap `AUTOMATION.md` calls the dialogue-to-*performance* half
    // of 2.2. This closes it from the script's side: the answer carries the
    // cast with every id a mouth needs, and the talk beats with their frames.
    host_fn!(ctx, host, state, "directScene", |state, story: String| {
        let mut s = state.borrow_mut();
        let directed = buzz_act::direct(s.scene_mut(), &story)
            .map_err(|e| throw(&format!("{e}")))?;

        let cast: Vec<Value> = directed
            .staged
            .puppets
            .iter()
            .enumerate()
            .map(|(i, puppet)| {
                let mut entry = puppet_json(puppet);
                if let Some(name) = directed.names.get(i) {
                    entry["name"] = json!(name);
                }
                entry["actor"] = json!(i);
                entry
            })
            .collect();

        let talking: Vec<Value> = directed
            .beats
            .iter()
            .filter(|beat| beat.action == buzz_act::perform::Action::Talk)
            .map(|beat| {
                json!({
                    "actor": beat.actor,
                    "from": beat.frames.start,
                    "to": beat.frames.end,
                })
            })
            .collect();

        Ok(json!({
            "frames": directed.frames,
            "cast": cast,
            "talking": talking,
            "ignored": directed.ignored,
            "message": directed.message,
        })
        .to_string())
    });

    // A performance with its size and its travel, where `perform` takes the
    // defaults: how big it is, and how far it moves.
    host_fn!(
        ctx,
        host,
        state,
        "performFully",
        |state, object: u64, action: String, from: u32, to: u32, amount: f64, distance: f64| {
            let action = action_named(&action)?;
            if to <= from {
                return Err(throw("a performance needs at least one frame"));
            }
            let mut performance = buzz_act::perform::Performance::new(action, from..to);
            if amount > 0.0 {
                performance.amount = amount;
            }
            if distance != 0.0 {
                performance.distance = distance;
            }
            let mut s = state.borrow_mut();
            buzz_act::perform::apply(s.scene_mut(), ObjectId(object), &performance)
                .map(|report| report.keyframes as i32)
                .map_err(|e| throw(&format!("{e}")))
        }
    );

    // **A live modifier on one named object**, rather than on the selection.
    //
    // The selection is the editor's idea of what a person is pointing at. A
    // script that has just built a character already knows which object it
    // wants, and making it select the thing first is ceremony.
    host_fn!(
        ctx,
        host,
        state,
        "addModifierTo",
        |state, object: u64, kind: String, options: String| {
            let options = parse_json(&options)?;
            let get = |name: &str, fallback: f64| number(&options, name).unwrap_or(fallback);
            let modifier = match kind.to_ascii_lowercase().as_str() {
                "breathe" => Modifier::Breathe {
                    rate: get("rate", 14.0),
                    depth: get("depth", 1.0),
                },
                "blink" => Modifier::Blink {
                    rate: get("rate", 12.0),
                    duration: get("duration", 0.16),
                },
                "sway" => Modifier::Sway {
                    amount: get("amount", 0.15),
                    rate: get("rate", 0.2),
                },
                "drift" => Modifier::Drift {
                    dx: get("dx", 10.0),
                    dy: get("dy", 0.0),
                    // How far it travels before it starts again, and how far
                    // into that loop it already is. The phase is what makes a
                    // *field* of drifting things possible rather than a queue
                    // of them crossing the sky in formation.
                    span: get("span", get("wrap", 0.0)),
                    phase: get("phase", get("start", 0.0)),
                },
                "wiggle" => Modifier::Wiggle {
                    amplitude: get("amplitude", 4.0),
                    frequency: get("frequency", 1.5),
                },
                "spring" => Modifier::Spring {
                    root: options.get("root").and_then(Value::as_u64).unwrap_or(0) as usize,
                    stiffness: get("stiffness", 90.0),
                    damping: get("damping", 8.0),
                    coupling: get("coupling", 0.5),
                },
                "lookat" => Modifier::LookAt {
                    x: get("x", 0.0),
                    y: get("y", 0.0),
                },
                "turn" => Modifier::Turn {
                    round: get("round", 0.0),
                },
                "squash" => Modifier::AutoSquashStretch {
                    amount: get("amount", 0.3),
                },
                other => {
                    return Err(throw(&format!(
                        "unknown modifier {other:?}; expected breathe, blink, sway, drift, \
                         wiggle, spring, lookAt, turn or squash"
                    )));
                }
            };
            let mut s = state.borrow_mut();
            let found = s
                .scene_mut()
                .update_object_across(0, u32::MAX, ObjectId(object), |o| {
                    o.modifiers.push(modifier);
                });
            if !found {
                return Err(throw(&format!("there is no object {object}")));
            }
            Ok(())
        }
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Rigging
// ---------------------------------------------------------------------------

fn rigging<'js>(
    ctx: &Ctx<'js>,
    host: &JsObject<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    // **Layer parenting** -- Animate's Parent column, and the half of a puppet
    // rig that is not bones. Pass `0` as the parent to unlink.
    host_fn!(
        ctx,
        host,
        state,
        "parentLayer",
        |state, child: u64, parent: u64, frame: u32| {
            let mut s = state.borrow_mut();
            let parent = (parent != 0).then_some(LayerId(parent));
            if s.scene().layers().get(LayerId(child)).is_none() {
                return Err(throw(&format!("there is no layer {child} to parent")));
            }
            Ok(s.scene_mut().set_follows(LayerId(child), parent, frame))
        }
    );

    // **A new layer, and its id back.**
    //
    // JSFL's `addNewLayer` answers with nothing, because in Animate you go and
    // look at the timeline afterwards. A script has no timeline to look at, and
    // the layer it just made is the one it is about to draw a mask on.
    host_fn!(
        ctx,
        host,
        state,
        "newLayer",
        |state, name: String, kind: String, depth: f64| {
            let kind = layer_kind_named(&kind)?;
            let mut s = state.borrow_mut();
            let name = if name.is_empty() {
                format!("Layer_{}", s.scene().layers().len() + 1)
            } else {
                name
            };
            let id = s.scene_mut().add_stage_layer(name, kind);
            if depth != 0.0 && depth.is_finite() {
                s.scene_mut().update_layer(id, |l| l.depth = depth);
            }
            // As long as the shot already is, so anything drawn on it lasts.
            let last = s.scene().frame_count().saturating_sub(1);
            s.scene_mut().update_layer(id, |l| {
                if l.frames.length() <= last {
                    l.frames.insert_frame(last);
                }
            });
            Ok(id.0)
        }
    );

    // A layer's kind: `normal`, `mask`, `masked`, `inverseMask`, `folder`,
    // `guide` or `guided`. A mask clips the run of masked layers under it.
    host_fn!(
        ctx,
        host,
        state,
        "setLayerKindOf",
        |state, layer: u64, kind: String| {
            let kind = layer_kind_named(&kind)?;
            let mut s = state.borrow_mut();
            if !s.scene_mut().update_layer(LayerId(layer), |l| l.kind = kind) {
                return Err(throw(&format!("there is no layer {layer}")));
            }
            Ok(())
        }
    );

    // The ids of the layers, front to back -- so a script can find what
    // staging built without counting rows.
    host_fn!(ctx, host, state, "layerIds", |state| {
        let s = state.borrow();
        Ok(json!(
            s.scene()
                .layers()
                .iter()
                .map(|l| json!({"id": l.id.0, "name": l.name,
                                "kind": l.kind.display_name(),
                                "follows": l.follows.map(|f| f.0)}))
                .collect::<Vec<_>>()
        )
        .to_string())
    });

    // Move a layer to a row, counted from the front. What `reorder_stage_layer`
    // does, and what putting a mask directly over the thing it masks needs.
    host_fn!(
        ctx,
        host,
        state,
        "moveLayerTo",
        |state, layer: u64, index: i32| {
            let mut s = state.borrow_mut();
            Ok(s.scene_mut()
                .reorder_stage_layer(LayerId(layer), index.max(0) as usize))
        }
    );

    // Where a layer sits now, front first, or `-1` if it is not on the stage.
    host_fn!(ctx, host, state, "layerRow", |state, layer: u64| {
        let s = state.borrow();
        Ok(s.scene()
            .stage_layer_index(LayerId(layer))
            .map(|i| i as i32)
            .unwrap_or(-1))
    });

    // **Draw on a named layer at a named frame**, which the JSFL `addRectangle`
    // cannot: it draws on whatever the editor has selected, and a script
    // building three scenes has no editor to select with.
    host_fn!(
        ctx,
        host,
        state,
        "addRectangleOn",
        |state, layer: u64, frame: u32, l: f64, t: f64, r: f64, b: f64, fill: String| {
            let colour = parse_color(&fill)?;
            let rect = buzz_geom::Rect::new(l.min(r), t.min(b), l.max(r), t.max(b));
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                return Err(throw("a shape needs a non-zero width and height"));
            }
            let mut s = state.borrow_mut();
            use buzz_geom::Shape as _;
            let shape = buzz_scene::ShapeData::filled(rect.to_path(1e-9), colour);
            s.scene_mut()
                .add_shape_at(LayerId(layer), frame, shape)
                .map(|id| id.0)
                .ok_or_else(|| throw(&format!("could not draw on layer {layer}")))
        }
    );

    // The same, as an oval. Two calls rather than one with a flag, because
    // rquickjs binds seven parameters to a native function and a shape on a
    // named layer at a named frame has used every one of them.
    host_fn!(
        ctx,
        host,
        state,
        "addOvalOn",
        |state, layer: u64, frame: u32, l: f64, t: f64, r: f64, b: f64, fill: String| {
            let colour = parse_color(&fill)?;
            let rect = buzz_geom::Rect::new(l.min(r), t.min(b), l.max(r), t.max(b));
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                return Err(throw("a shape needs a non-zero width and height"));
            }
            let mut s = state.borrow_mut();
            use buzz_geom::Shape as _;
            let path = kurbo::Ellipse::from_rect(rect).to_path(1e-3);
            let shape = buzz_scene::ShapeData::filled(path, colour);
            s.scene_mut()
                .add_shape_at(LayerId(layer), frame, shape)
                .map(|id| id.0)
                .ok_or_else(|| throw(&format!("could not draw on layer {layer}")))
        }
    );

    // **A procedural texture on a shape** -- paper, noise, grass, brick.
    //
    // The tile is baked into the document's image library once per recipe and
    // shared, exactly as the Colour panel bakes it, so texturing twenty shapes
    // costs one tile.
    host_fn!(
        ctx,
        host,
        state,
        "textureObject",
        |state, object: u64, kind: String, fg: String, bg: String, detail: u32, cell: f64| {
            let recipe = buzz_scene::TextureRecipe {
                kind: texture_named(&kind)?,
                fg: parse_color(&fg)?,
                bg: parse_color(&bg)?,
                detail: detail.max(1),
                contrast: 1.0,
            };
            let mut s = state.borrow_mut();
            let asset = match s.scene().images().find_by_recipe(&recipe) {
                Some(existing) => existing,
                None => {
                    let name = s.scene().images().unique_name(recipe.kind.label());
                    let id = s.scene_mut().next_image_id();
                    let asset = buzz_scene::ImageAsset::from_recipe(id, name, recipe, 256);
                    s.scene_mut().images_mut().insert(asset)
                }
            };
            let cell = if cell > 0.0 { cell } else { 128.0 };
            let fill = buzz_scene::ImageFill::tiled(asset, cell);
            let touched = s
                .scene_mut()
                .update_object_across(0, u32::MAX, ObjectId(object), |o| {
                    if let buzz_scene::ObjectKind::Shape(shape) = &mut o.kind {
                        shape.fill = Some(buzz_scene::FillSpec::image(fill.clone()));
                    }
                });
            if !touched {
                return Err(throw(&format!("there is no object {object} to texture")));
            }
            Ok(())
        }
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Dialogue
// ---------------------------------------------------------------------------

fn dialogue<'js>(
    ctx: &Ctx<'js>,
    host: &JsObject<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    // The tracks the host decoded on the way in.
    host_fn!(ctx, host, state, "soundCount", |state| {
        Ok(state.borrow().sounds.len() as i32)
    });
    host_fn!(ctx, host, state, "soundInfo", |state, index: i32| {
        let s = state.borrow();
        let clip = s
            .sounds
            .get(index.max(0) as usize)
            .ok_or_else(|| throw(&format!("there is no sound {index}")))?;
        Ok(json!({
            "name": clip.name,
            "seconds": clip.duration_seconds(),
            "sampleRate": clip.sample_rate,
            "channels": clip.channels,
        })
        .to_string())
    });

    // **Put one of them on the timeline.** A sound layer of its own, with the
    // cue on the frame asked for, because that is where the exporter reads a
    // mix from.
    host_fn!(
        ctx,
        host,
        state,
        "attachSound",
        |state, index: i32, name: String, frame: u32, volume: f64| {
            let clip = {
                let s = state.borrow();
                s.sounds
                    .get(index.max(0) as usize)
                    .cloned()
                    .ok_or_else(|| throw(&format!("there is no sound {index}")))?
            };
            let mut s = state.borrow_mut();
            let label = if name.is_empty() {
                clip.name.clone()
            } else {
                name
            };
            // **Re-encoded as a WAV rather than carried as the original file.**
            // The host may have handed over a *slice* of a track -- fifteen
            // seconds out of four minutes -- and there is no way to say that in
            // an mp3 without re-encoding it. The samples are what the analysis
            // and the mix both read, so the samples are what goes in.
            let (bytes, rate, channels, length) = wav_of(&clip);
            let sound = s
                .scene_mut()
                .add_sound(&label, bytes, "wav", rate, channels, length);
            let layer = s
                .scene_mut()
                .add_stage_layer(format!("{label} (sound)"), LayerKind::Normal);
            let fps = s.scene().stage().frame_rate.max(1.0);
            let last = frame + clip.duration_frames(fps);
            s.scene_mut().update_stage_layer(layer, |l| {
                while l.frames.length() <= last {
                    l.frames.insert_frame(l.frames.length());
                }
                l.frames.insert_keyframe(frame);
            });
            let mut reference = buzz_scene::SoundRef::stream(sound);
            if volume > 0.0 {
                reference.volume = volume.clamp(0.0, 1.0) as f32;
            }
            if !s.scene_mut().set_frame_sound(layer, frame, Some(reference)) {
                return Err(throw("the sound had no keyframe to attach to"));
            }
            Ok(json!({"sound": sound.0, "layer": layer.0, "frames": clip.duration_frames(fps)})
                .to_string())
        }
    );

    // **Lip sync one character against one track.**
    //
    // The whole of `File > Lip Sync`, minus the dialog: analyse the clip into
    // visemes, and write a keyframe per mouth shape onto that character's own
    // mouth layer, placed on their own face.
    host_fn!(ctx, host, state, "lipSync", |state, options: String| {
        let options = parse_json(&options)?;
        let index = options.get("sound").and_then(Value::as_u64).unwrap_or(0) as usize;
        let clip = {
            let s = state.borrow();
            s.sounds
                .get(index)
                .cloned()
                .ok_or_else(|| throw(&format!("there is no sound {index}")))?
        };
        let layer = LayerId(options.get("layer").and_then(Value::as_u64).unwrap_or(0));
        let mouth = SymbolId(options.get("mouth").and_then(Value::as_u64).unwrap_or(0));
        let start = options.get("start").and_then(Value::as_u64).unwrap_or(0) as u32;
        let at = Point::new(
            number(&options, "x").unwrap_or(0.0),
            number(&options, "y").unwrap_or(0.0),
        );
        let scale = number(&options, "scale").unwrap_or(1.0);
        let settings = buzz_audio::LipSyncOptions {
            silence: number(&options, "silence")
                .map(|v| v as f32)
                .unwrap_or(buzz_audio::LipSyncOptions::default().silence),
            hold: options
                .get("hold")
                .and_then(Value::as_u64)
                .map(|v| v as u32)
                .unwrap_or(buzz_audio::LipSyncOptions::default().hold),
        };
        let placement = Affine::translate(at.to_vec2()) * Affine::scale(scale);

        let mut s = state.borrow_mut();
        let report = buzz_act::lipsync::apply(
            s.scene_mut(),
            &clip,
            start,
            layer,
            mouth,
            placement,
            &settings,
        )
        .map_err(|e| throw(&format!("{e}")))?;
        Ok(json!({
            "keyframes": report.keyframes,
            "frames": report.frames,
            "silent": report.silent,
            "message": report.message,
        })
        .to_string())
    });

    // Where the voice speaks and where it breathes -- the phrase detection
    // `Fit to Narration` uses, so a script can cut on a pause.
    host_fn!(ctx, host, state, "phrasesIn", |state, index: i32| {
        let s = state.borrow();
        let clip = s
            .sounds
            .get(index.max(0) as usize)
            .ok_or_else(|| throw(&format!("there is no sound {index}")))?;
        let fps = s.scene().stage().frame_rate.max(1.0);
        let phrases = buzz_audio::detect_phrases(clip, fps, &buzz_audio::PhraseOptions::default());
        Ok(json!(
            phrases
                .iter()
                .map(|p| json!({"start": p.start, "end": p.end}))
                .collect::<Vec<_>>()
        )
        .to_string())
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Library and assets
// ---------------------------------------------------------------------------

fn library<'js>(
    ctx: &Ctx<'js>,
    host: &JsObject<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    // A symbol by name, or `0`. How a script **recalls** what an earlier scene
    // filed rather than drawing it a second time.
    host_fn!(ctx, host, state, "findSymbol", |state, name: String| {
        let s = state.borrow();
        Ok(s.scene()
            .library()
            .find_by_name(&name)
            .map(|symbol| symbol.id.0)
            .unwrap_or(0))
    });

    // Place an instance. `frameOfSymbol` shows one drawing and holds there --
    // which is what a mouth shape and a turnaround view both are; pass `-1`
    // for a symbol that should play.
    host_fn!(
        ctx,
        host,
        state,
        "placeSymbol",
        |state,
         symbol: u64,
         layer: u64,
         frame: u32,
         x: f64,
         y: f64,
         scale: f64,
         frame_of_symbol: i32| {
            let mut s = state.borrow_mut();
            let scale = if scale != 0.0 { scale } else { 1.0 };
            let at = Affine::translate((x, y)) * Affine::scale(scale);
            let id = s
                .scene_mut()
                .add_instance_at(LayerId(layer), frame, SymbolId(symbol), at)
                .ok_or_else(|| {
                    throw(&format!(
                        "could not place symbol {symbol} on layer {layer} at frame {frame}"
                    ))
                })?;
            if frame_of_symbol >= 0 {
                s.scene_mut().update_object_at(frame, id, |o| {
                    if let buzz_scene::ObjectKind::Instance(instance) = &mut o.kind {
                        instance.first_frame = frame_of_symbol as u32;
                        instance.loop_mode = buzz_scene::LoopMode::SingleFrame;
                    }
                });
            }
            Ok(id.0)
        }
    );

    // **Lift objects into a symbol**, as `Modify > Convert to Symbol` does,
    // but naming the objects rather than taking the selection.
    host_fn!(
        ctx,
        host,
        state,
        "symbolFromObjects",
        |state, name: String, kind: String, ids: String, layer: u64, frame: u32| {
            let ids: Vec<u64> = serde_json::from_str(&ids)
                .map_err(|e| throw(&format!("the object list is not JSON: {e}")))?;
            if ids.is_empty() {
                return Err(throw("a symbol needs at least one object in it"));
            }
            let kind = symbol_kind_named(&kind)?;
            let mut s = state.borrow_mut();
            let symbol = s.scene_mut().add_symbol(name, kind, None);
            let Some(inner) = s
                .scene()
                .library()
                .get(symbol)
                .and_then(|x| x.layers.iter().next())
                .map(|l| l.id)
            else {
                return Err(throw("the new symbol has no layer to fill"));
            };
            let mut lifted = Vec::new();
            for id in ids {
                if let Some(object) = s.scene_mut().remove_object(ObjectId(id)) {
                    lifted.push(object);
                }
            }
            let count = lifted.len();
            s.scene_mut().library_mut().update(symbol, |sym| {
                sym.layers.update(inner, |l| {
                    l.frames.set_objects(0, lifted);
                });
            });
            let placed = s
                .scene_mut()
                .add_instance_at(LayerId(layer), frame, symbol, Affine::IDENTITY);
            Ok(json!({"symbol": symbol.0, "instance": placed.map(|p| p.0).unwrap_or(0),
                      "lifted": count})
            .to_string())
        }
    );

    // **File a symbol in the asset library on disk**, so it outlives the
    // document. The same folder the Assets panel shows, and the only path a
    // script may write to.
    host_fn!(
        ctx,
        host,
        state,
        "saveAsset",
        |state, symbol: u64, name: String, folder: String| {
            let (root, extracted) = {
                let s = state.borrow();
                let Some(root) = s.context.asset_root.clone() else {
                    return Err(throw(
                        "no asset library is available on this machine, so nothing can be filed",
                    ));
                };
                let extracted = s.scene().extract_symbol(SymbolId(symbol)).ok_or_else(|| {
                    throw(&format!("there is no symbol {symbol} to file as an asset"))
                })?;
                (root, extracted)
            };
            let mut library = buzz_doc::AssetLibrary::at(root);
            library.rescan();
            let asset = library
                .save(&name, folder.trim(), &extracted)
                .map_err(|e| throw(&format!("{e}")))?;
            Ok(json!({"name": asset.name, "folder": asset.folder}).to_string())
        }
    );

    // **Bring one back**, into the scene the script is on. The recall half of
    // the pair above, and what makes a cast survive from one film to the next.
    host_fn!(
        ctx,
        host,
        state,
        "placeAsset",
        |state, name: String, folder: String| {
            let root = {
                let s = state.borrow();
                s.context.asset_root.clone().ok_or_else(|| {
                    throw("no asset library is available on this machine")
                })?
            };
            let mut library = buzz_doc::AssetLibrary::at(root);
            library.rescan();
            let folder = folder.trim().to_string();
            let asset = library
                .assets()
                .iter()
                .find(|a| a.name == name && a.folder == folder)
                .cloned()
                .ok_or_else(|| throw(&format!("there is no asset called {name:?}")))?;
            let mut s = state.borrow_mut();
            let report = library
                .place(&asset, s.scene_mut())
                .map_err(|e| throw(&format!("{e}")))?;
            Ok(json!({"layers": report.layers, "symbols": report.symbols}).to_string())
        }
    );

    // What is in the asset library, so a script can ask before it draws.
    host_fn!(ctx, host, state, "assetNames", |state| {
        let root = { state.borrow().context.asset_root.clone() };
        let Some(root) = root else {
            return Ok("[]".to_string());
        };
        let mut library = buzz_doc::AssetLibrary::at(root);
        library.rescan();
        Ok(json!(
            library
                .assets()
                .iter()
                .map(|a| json!({"name": a.name, "folder": a.folder}))
                .collect::<Vec<_>>()
        )
        .to_string())
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

fn camera<'js>(
    ctx: &Ctx<'js>,
    host: &JsObject<'js>,
    state: &Rc<RefCell<State>>,
) -> JsResult<()> {
    // **A named camera move**, already eased -- push in, pull out, pan left or
    // right, reveal, drift. The eased pair of keys `Camera > Move` writes.
    host_fn!(
        ctx,
        host,
        state,
        "cameraMove",
        |state, kind: String, from: u32, to: u32| {
            let movement = match kind.to_ascii_lowercase().replace(' ', "").as_str() {
                "pushin" | "push" => buzz_scene::CameraMove::PushIn,
                "pullout" | "pull" => buzz_scene::CameraMove::PullOut,
                "panleft" => buzz_scene::CameraMove::PanLeft,
                "panright" => buzz_scene::CameraMove::PanRight,
                "reveal" => buzz_scene::CameraMove::Reveal,
                "drift" => buzz_scene::CameraMove::Drift,
                other => {
                    return Err(throw(&format!(
                        "unknown camera move {other:?}; expected pushIn, pullOut, panLeft, \
                         panRight, reveal or drift"
                    )));
                }
            };
            let mut s = state.borrow_mut();
            let stage = s.scene().stage().stage_rect();
            let camera = s.scene_mut().camera_mut();
            camera.enabled = true;
            Ok(camera.add_move(movement, from, to, stage))
        }
    );

    // A camera key with an ease of its own -- `linear`, `smooth`, `easeIn` or
    // `easeOut`.
    host_fn!(
        ctx,
        host,
        state,
        "setEasedCameraKey",
        |state, frame: u32, x: f64, y: f64, zoom: f64, rotation: f64, ease: String| {
            let easing = match ease.to_ascii_lowercase().replace(' ', "").as_str() {
                "" | "linear" => buzz_scene::Easing::Linear,
                "smooth" | "easeinout" => buzz_scene::camera_track::SMOOTH,
                "easein" | "slowin" => buzz_scene::Easing::Strength(-60.0),
                "easeout" | "slowout" => buzz_scene::Easing::Strength(60.0),
                other => return Err(throw(&format!("unknown ease {other:?}"))),
            };
            let mut s = state.borrow_mut();
            let camera = s.scene_mut().camera_mut();
            camera.enabled = true;
            let mut key = buzz_scene::CameraKey::new(frame, Point::new(x, y));
            key.zoom = if zoom > 0.0 { zoom } else { 1.0 };
            key.rotation = rotation.to_radians();
            key.ease = easing;
            camera.set_key(key.clamped());
            Ok(())
        }
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn puppet_json(puppet: &buzz_act::puppet::Puppet) -> Value {
    json!({
        "bodyLayer": puppet.body_layer.0,
        "body": puppet.body.0,
        "faceLayer": puppet.face_layer.0,
        "mouthLayer": puppet.mouth_layer.0,
        "eyes": puppet.eyes.0,
        "brows": puppet.brows.0,
        "eyesSymbol": puppet.eyes_symbol.0,
        "browsSymbol": puppet.brows_symbol.0,
        "mouthSymbol": puppet.mouth_symbol.0,
        "mouthX": puppet.mouth_at.x,
        "mouthY": puppet.mouth_at.y,
        "mouthScale": puppet.mouth_scale,
        "headX": puppet.head.x,
        "headY": puppet.head.y,
    })
}

fn parse_json(text: &str) -> JsResult<Value> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(text).map_err(|e| throw(&format!("that is not JSON: {e}")))
}

fn number(options: &Value, name: &str) -> Option<f64> {
    options.get(name).and_then(Value::as_f64)
}

fn colour(options: &Value, name: &str) -> JsResult<Option<peniko::Color>> {
    match options.get(name).and_then(Value::as_str) {
        Some(text) => parse_color(text).map(Some),
        None => Ok(None),
    }
}

/// A WAV of a clip's own samples.
///
/// See `attachSound`: the document stores the bytes it was given, and what the
/// host handed over may be a slice of a longer file rather than the file.
fn wav_of(clip: &buzz_audio::Clip) -> (std::sync::Arc<Vec<u8>>, u32, u16, u64) {
    let channels = clip.channels.max(1);
    let rate = clip.sample_rate.max(1);
    let frames = clip.len();
    let data_bytes = (clip.samples.len() * 2) as u32;

    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in clip.samples.iter() {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    (std::sync::Arc::new(out), rate, channels, frames as u64)
}

fn setting_named(text: &str) -> JsResult<buzz_act::staging::Setting> {
    use buzz_act::staging::Setting;
    Ok(match text.to_ascii_lowercase().as_str() {
        "daylight" | "day" => Setting::Daylight,
        "sunset" | "dusk" => Setting::Sunset,
        "night" => Setting::Night,
        "interior" | "inside" => Setting::Interior,
        "storm" => Setting::Storm,
        other => {
            return Err(throw(&format!(
                "unknown setting {other:?}; expected daylight, sunset, night, interior or storm"
            )));
        }
    })
}

fn scenery_named(text: &str) -> JsResult<buzz_act::scenery::Scenery> {
    use buzz_act::scenery::Scenery;
    Ok(match text.to_ascii_lowercase().as_str() {
        "bare" | "none" => Scenery::Bare,
        "forest" => Scenery::Forest,
        "city" => Scenery::City,
        "village" => Scenery::Village,
        "meadow" => Scenery::Meadow,
        "waterside" | "river" => Scenery::Waterside,
        other => {
            return Err(throw(&format!(
                "unknown scenery {other:?}; expected forest, village, city, meadow or waterside"
            )));
        }
    })
}

fn effect_named(text: &str) -> JsResult<buzz_scene::EffectKind> {
    use buzz_scene::EffectKind;
    Ok(match text.to_ascii_lowercase().replace(' ', "").as_str() {
        "snow" => EffectKind::Snow,
        "rain" => EffectKind::Rain,
        "stars" => EffectKind::Stars,
        "fireflies" => EffectKind::Fireflies,
        "bokeh" => EffectKind::Bokeh,
        "clouds" => EffectKind::Clouds,
        "diffusedlight" => EffectKind::DiffusedLight,
        "lightrays" => EffectKind::LightRays,
        "moonlight" => EffectKind::Moonlight,
        "stringlights" => EffectKind::StringLights,
        "lamps" => EffectKind::Lamps,
        "buildings" => EffectKind::Buildings,
        "pinetrees" => EffectKind::PineTrees,
        "leafytrees" => EffectKind::LeafyTrees,
        "grass" => EffectKind::Grass,
        other => return Err(throw(&format!("unknown effect brush {other:?}"))),
    })
}

fn texture_named(text: &str) -> JsResult<buzz_scene::TextureKind> {
    use buzz_scene::TextureKind;
    for kind in TextureKind::ALL {
        if kind.label().eq_ignore_ascii_case(text) {
            return Ok(kind);
        }
    }
    Err(throw(&format!("unknown texture {text:?}")))
}

fn layer_kind_named(text: &str) -> JsResult<LayerKind> {
    Ok(match text.to_ascii_lowercase().replace(' ', "").as_str() {
        "normal" => LayerKind::Normal,
        "folder" => LayerKind::Folder,
        "mask" => LayerKind::Mask,
        "inversemask" => LayerKind::InverseMask,
        "masked" => LayerKind::Masked,
        "guide" => LayerKind::Guide,
        "guided" => LayerKind::Guided,
        other => {
            return Err(throw(&format!(
                "unknown layer kind {other:?}; expected normal, folder, mask, inverseMask, \
                 masked, guide or guided"
            )));
        }
    })
}

fn symbol_kind_named(text: &str) -> JsResult<SymbolKind> {
    Ok(match text.to_ascii_lowercase().replace(' ', "").as_str() {
        "graphic" | "" => SymbolKind::Graphic,
        "button" => SymbolKind::Button,
        "movieclip" | "movie" => SymbolKind::MovieClip,
        other => return Err(throw(&format!("unknown symbol type {other:?}"))),
    })
}

fn action_named(text: &str) -> JsResult<buzz_act::perform::Action> {
    use buzz_act::perform::Action;
    Ok(match text.to_ascii_lowercase().as_str() {
        "walk" => Action::Walk,
        "run" => Action::Run,
        "talk" => Action::Talk,
        "idle" => Action::Idle,
        "sit" => Action::Sit,
        "stand" => Action::Stand,
        "turn" => Action::Turn,
        "point" => Action::Point,
        "reach" => Action::Reach,
        "react" => Action::React,
        other => {
            return Err(throw(&format!(
                "unknown action {other:?}; expected walk, run, talk, idle, sit, stand, turn, \
                 point, reach or react"
            )));
        }
    })
}
