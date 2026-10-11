//! The AI agent's executor: runs any editor [`Command`] by its stable string id,
//! and answers `inspect` questions, through the same code a person's menus,
//! shortcuts and file dialogs use. See `docs/AI_AGENT_PLAN.md` (Stage A).
//!
//! Callable headlessly — `App::new` and no window — which is how it is tested.
//! It never opens a dialog and never blocks: a command that would need a modal
//! with no answer yet returns `needs_dialog <id>` so the caller can choose
//! another route.

use crate::app::{App, Pick};
use buzz_ui::{Command, ExportKind};

pub mod assets;
pub mod dialogs;
pub mod jobs;
pub mod lists;
pub mod props;
use serde_json::{Value, json};

/// One tool invocation from the model: `{tool, args}`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentCall {
    pub tool: String,
    pub args: Value,
}

/// What came back: errors are results, never panics.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentResult {
    pub ok: bool,
    pub value: Value,
    pub error: Option<String>,
}

impl AgentResult {
    fn ok(value: Value) -> Self {
        Self {
            ok: true,
            value,
            error: None,
        }
    }

    fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            value: Value::Null,
            error: Some(error.into()),
        }
    }
}

/// Commands the agent cannot run, with why. Everything else is either run
/// directly, run with a `path`, or answers `needs_dialog`.
pub const AGENT_EXCLUDED: &[(&str, &str)] = &[
    ("quit", "needs user confirmation: ends the program"),
    ("close", "needs user confirmation: may discard unsaved work"),
    ("shortcut_editor", "UI-only dialog, no document effect"),
    ("snapshots", "UI-only list dialog; use save_snapshot to keep one"),
    ("about", "UI-only dialog, no document effect"),
    ("ai_models", "UI-only dialog for installing models"),
    ("voice_push_to_talk", "opens the user's microphone: only the person at the keyboard may do that"),
    ("voice_mute", "controls the user's microphone privacy switch: not the agent's to flip"),
    ("pasteboard_colour", "UI-only colour dialog, no document effect"),
    ("test_movie", "full-screen playback overlay, no document effect"),
    ("run_script", "runs on a background thread from the Actions panel; the script tool (Stage C) replaces it"),
    ("buzzedit_send", "opens the Send to BuzzEdit dialog; the agent uses the send_to_buzzedit tool (and buzzedit_projects to pick a project)"),
    ("buzzedit_transcribe", "opens the Transcribe with BuzzEdit dialog; the agent uses the transcribe_with_buzzedit tool"),
    ("studio_import_script", "opens the Import Script from Studio dialog; the agent uses the studio_projects and studio_build_episode tools"),
];

/// How the agent reaches a command.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Dispatched straight through `App::dispatch`.
    Run,
    /// A file dialog's command: `args.path` is handed to the same handler the
    /// dialog's answer goes to.
    Path(Pick),
    /// Opens a modal or panel that wants parameters; refused until the
    /// parameters can be passed as args. What is left is in
    /// [`dialogs::STILL_NEEDS_DIALOG`], each with its reason.
    NeedsDialog,
    /// A dialog's command run from its args: they fill the dialog's state and
    /// the same function its OK button calls runs ([`dialogs`]).
    Args,
    /// Saves in place when the document has a file, else needs a path.
    Save,
    /// In [`AGENT_EXCLUDED`].
    Excluded,
}

/// Classify every command. An exhaustive `match`: a new `Command` variant fails
/// to compile here until it is routed (or excluded).
pub fn route(command: Command) -> Route {
    use Command::*;
    use buzz_scene::ImportTarget;
    match command {
        // File dialogs: the answer is a path.
        Open => Route::Path(Pick::Open),
        SaveAs => Route::Path(Pick::SaveAs),
        ImportToLibrary => Route::Path(Pick::ImportInto(ImportTarget::Library)),
        ImportToStage => Route::Path(Pick::ImportInto(ImportTarget::Stage)),
        ExportFla => Route::Path(Pick::ExportFla),
        ExportImage => Route::Path(Pick::Export(ExportKind::Image)),
        ExportSequence => Route::Path(Pick::Export(ExportKind::Sequence)),
        ExportVideo => Route::Path(Pick::Export(ExportKind::Video)),
        ExportGif => Route::Path(Pick::Export(ExportKind::Gif)),
        ExportWebp => Route::Path(Pick::Export(ExportKind::Webp)),
        ImportSound => Route::Path(Pick::ImportSound),
        ImportImage => Route::Path(Pick::ImportImage),
        ImportVideoReference => Route::Path(Pick::VideoReference),
        ImportSequenceFolder => Route::Path(Pick::SequenceFolder),
        SetMasterAudio => Route::Path(Pick::SetMasterAudio),
        ImportCaptions => Route::Path(Pick::ImportCaptions),
        ExportCaptions => Route::Path(Pick::ExportCaptions),
        AiSceneFromImage => Route::Path(Pick::AiSceneImage),

        Save => Route::Save,

        // Open a dialog or panel that needs parameters: run from args.
        New | LipSync | TraceBitmapOptions | SetScene | DirectScene | SceneryFor | Perform
        | AddFollowThrough | AddWiggle | AddTitleCard | AiIdentifyAssets | AiRigVectorCharacter
        | AiDirectStory | AiCharacterFromImage | AiMouthChart => Route::Args,

        // Needs the user, or has no document effect.
        Quit | Close | ShortcutEditor | Snapshots | About | AiModels | PasteboardColour
        | TestMovie | RunScript | VoicePushToTalk | VoiceMute => Route::Excluded,
        // Dialogs over BuzzEdit and Buzzcaf Studio; their tools take args.
        SendToBuzzEdit | TranscribeWithBuzzEdit | ImportScriptFromStudio => Route::Excluded,

        // Everything else runs as it does from the menu or the keyboard.
        SaveSnapshot | AttachSound | RemoveSound | NewMouthSymbol | DetectBeats | FitToNarration
        | LipSyncFromCaptions | Undo | Redo | Cut | Copy | Paste | Delete | SelectAll | Deselect
        | DuplicateSelection | ZoomIn | ZoomOut | ZoomActual | ZoomFitInWindow | ZoomShowFrame
        | ZoomShowAll | ToggleRulers | ToggleGrid | ToggleGuides | ToggleSnapping
        | TogglePasteboard | GroupSelection | UngroupSelection | BreakApart | BringToFront
        | BringForward | SendBackward | SendToBack | RetargetPerformance | SwapSymbol
        | PaintThrough | ExposeOnTwos | ExposeOnThrees | SelectSameColour | ThickenStroke
        | ThinStroke | TraceBitmap | TraceLineArt | ConvertLinesToFills | ExpandFill
        | SmoothSelection | StraightenSelection | RecogniseShape | FlipHorizontal | FlipVertical
        | RotateClockwise | RotateAnticlockwise | NewLayer | NewLayerFolder | MoveLayerUp
        | MoveLayerDown | NewReferenceLayer | DeleteLayer | ConvertToSymbol | NewSymbol
        | EditSymbol | EditDocument | PlaceInstance | DuplicateSymbol | SymbolToAsset
        | DeleteSymbol | NewLibraryFolder | BrushFromSelection | ToggleLayoutLock | ToggleTheme
        | ToggleReduceMotion | ResetWorkspace | SaveWorkspace | AddSun | AddSky | AddLamp
        | AddGloom | AddFire | AddStorm | ToggleLightGizmos | ToggleBoneRig | ToggleBoneMode
        | AddScene
        | DuplicateScene | AddPerson | ClearModifiers | BakeModifiers | SetReverse
        | ClearReverse | AddProfileRight | AddProfileLeft | AddThreeQuarterRight
        | AddThreeQuarterLeft | MakeSetupFromScene | SplitAtPlayhead | FlattenToMasterScene
        | ToggleActionsPanel | ClearScriptOutput | CreateClassicTween | CreateMotionTween
        | CreateShapeTween | RemoveTween | InsertFrame | RemoveFrame | InsertKeyframe
        | InsertBlankKeyframe | ClearKeyframe | PlayPause | NextFrame | PreviousFrame
        | FirstFrame | LastFrame | CutFrames | CopyFrames | PasteFrames | ClearFrames
        | ReverseFrames | ToggleOnionSkin | ToggleAutoKeyframe | ToggleEditMultipleFrames
        | ToggleCamera | AddCameraKeyframe | RemoveCameraKeyframe | ResetCamera
        | AddLightKeyframe | RemoveLightKeyframe | SaveAsTemplate => Route::Run,

        // Data-carrying: the args select the value.
        TogglePanel(_) | SetTheme(_) | SetUiScale(_) | SetWorkspacePreset(_)
        | SetMasterAudioFromLayer(_) | NewShotFromSetup(_) | TryAnotherSetup(..)
        | DuplicateShot(_) | AddMissingSetupLayers(_) | EditSetup(_) | CutOnDialogue(_)
        | ContinueFromShot(..) | AddCameraMove(_) | SelectTool(_) | NewFromTemplate(_)
        | Align { .. } | Distribute(_) | MatchSize(_) | Nudge { .. }
        | MoveFrames { .. } => Route::Run,
    }
}

/// Run one tool call against the app. Never panics on bad input.
pub fn execute(app: &mut App, call: &AgentCall) -> AgentResult {
    match call.tool.as_str() {
        "command" => run_command(app, &call.args),
        "inspect" => inspect(app, &call.args),
        "gesture" => gesture(app, &call.args),
        "set_style" => set_style(app, &call.args),
        "script" => script(app, &call.args),
        "snapshot" => snapshot(app, &call.args),
        "set_property" => props::set_property(app, &call.args),
        "edit_list" => lists::edit_list(app, &call.args),
        "wait_for_jobs" => jobs::wait_for_jobs(app, &call.args),
        "transcribe" => jobs::transcribe(app, &call.args),
        "arrange_assets" => assets::arrange_assets(app, &call.args),
        "dress_set_from_assets" => assets::dress_set_from_assets(app, &call.args),
        // BuzzEdit and Buzzcaf Studio (`crate::estate`, plan steps E2 and E3).
        "send_to_buzzedit" | "buzzedit_projects" | "transcribe_with_buzzedit" | "studio_projects"
        | "studio_build_episode" => crate::estate::tool(app, &call.tool, &call.args),
        other => AgentResult::err(format!("unknown tool '{other}'")),
    }
}

/// The arguments object for a command call: `args.args` when given, else the
/// call's own args (so `{name, path}` and `{name, args:{path}}` both work).
fn command_args(args: &Value) -> &Value {
    match args.get("args") {
        Some(inner) if inner.is_object() => inner,
        _ => args,
    }
}

/// "Allow all" unlocks the excluded commands except quit and close: the app
/// has no unsaved-changes prompt on those paths (`ClosePrompt` is declared but
/// never raised), so an unlocked quit could silently discard work.
fn allow_all_unlocks(id: &str) -> bool {
    !matches!(id, "quit" | "close")
}

fn run_command(app: &mut App, args: &Value) -> AgentResult {
    let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
        return AgentResult::err("command needs a 'name'");
    };
    let inner = command_args(args);
    let command = match Command::from_id(name, inner) {
        Ok(c) => c,
        Err(e) => return AgentResult::err(e),
    };
    let id = command.id();

    match route(command) {
        Route::Excluded if app.agent_allow_all() && allow_all_unlocks(id) => {
            // "Allow all": run it as the menu would.
            app.agent_editor_mut().status = None;
            app.dispatch(command);
            return AgentResult::ok(report(app, id));
        }
        Route::Excluded => {
            let reason = AGENT_EXCLUDED
                .iter()
                .find(|(excluded, _)| *excluded == id)
                .map(|(_, r)| *r)
                .unwrap_or("not available to the agent");
            return AgentResult::err(format!("excluded {id}: {reason}"));
        }
        Route::NeedsDialog => {
            return match dialogs::STILL_NEEDS_DIALOG.iter().find(|(c, _)| *c == id) {
                Some((_, why)) => AgentResult::err(format!("needs_dialog {id}: {why}")),
                None => AgentResult::err(format!("needs_dialog {id}")),
            };
        }
        Route::Args => return dialogs::run(app, command, inner),
        Route::Save => {
            if app.agent_editor().doc.path().is_none() {
                return AgentResult::err(format!("needs_dialog {id}: the document has no file yet; use save_as with a path"));
            }
        }
        Route::Path(pick) => {
            let path = inner
                .get("path")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str());
            let Some(path) = path else {
                return AgentResult::err(format!("needs_dialog {id}: pass args.path"));
            };
            app.agent_editor_mut().status = None;
            app.agent_forget_last_task();
            app.apply_pick(pick, std::path::PathBuf::from(path));
            let mut value = report(app, id);
            // Scene from Image starts an AI task: hand the model its job id.
            if let Some(job) = app.agent_track_last_job(id) {
                value["started"] = json!(true);
                value["job"] = json!(job);
                value["note"] = json!(jobs::WAIT_NOTE);
            }
            return AgentResult::ok(value);
        }
        Route::Run => {}
    }

    app.agent_editor_mut().status = None;
    app.dispatch(command);
    AgentResult::ok(report(app, id))
}

/// What a command left behind: its id, the status line it set, and history.
fn report(app: &App, id: &str) -> Value {
    let editor = app.agent_editor();
    json!({
        "id": id,
        "status": editor.status,
        "can_undo": editor.doc.can_undo(),
        "undo_depth": editor.doc.history().undo_depth(),
    })
}

fn inspect(app: &App, args: &Value) -> AgentResult {
    let Some(what) = args.get("what").and_then(|v| v.as_str()) else {
        return AgentResult::err("inspect needs 'what': document|timeline|objects|selection|library|assets|tools|properties|catalog");
    };
    if what == "properties" {
        return props::inspect_properties(app, args);
    }
    let editor = app.agent_editor();
    let scene = editor.doc.scene();
    let value = match what {
        "document" => {
            let size = scene.stage().size;
            json!({
                "title": editor.doc.title(),
                "path": editor.doc.path().map(|p| p.display().to_string()),
                "dirty": editor.doc.is_dirty(),
                "width": size.width,
                "height": size.height,
                "fps": scene.stage().frame_rate,
                "frame_count": scene.frame_count(),
                "scenes": editor.doc.scene_names(),
                "active_scene": editor.doc.active_scene(),
                "can_undo": editor.doc.can_undo(),
                "can_redo": editor.doc.can_redo(),
                "undo_depth": editor.doc.history().undo_depth(),
            })
        }
        "timeline" => {
            // **`index` is what `layer:<n>` means** everywhere a target is
            // named (`set_property`, `edit_list`). The id is a different
            // number once layers have been added; given only the id, a live
            // model renamed the wrong layer.
            let layers: Vec<Value> = scene
                .layers()
                .iter()
                .enumerate()
                .map(|(index, layer)| {
                    let keys: Vec<Value> = layer
                        .frames
                        .keyframes()
                        .iter()
                        .map(|k| {
                            json!({
                                "start": k.start,
                                "label": k.label,
                                "objects": k.objects.len(),
                                "tween": format!("{:?}", k.tween.kind),
                            })
                        })
                        .collect();
                    json!({
                        "index": index,
                        "id": layer.id.0,
                        "name": layer.name,
                        "kind": format!("{:?}", layer.kind),
                        "visible": layer.visible,
                        "locked": layer.locked,
                        "parent": layer.parent.map(|p| p.0),
                        "length": layer.frames.length(),
                        "keyframes": keys,
                    })
                })
                .collect();
            json!({
                "playhead": editor.current_frame,
                "playing": editor.playback.playing,
                "frame_count": scene.frame_count(),
                "active_layer": editor.selection.active_layer().map(|l| l.0),
                "active_layer_index": editor
                    .selection
                    .active_layer()
                    .and_then(|a| scene.layers().iter().position(|l| l.id == a)),
                "layers": layers,
                // **The lights' own keyframes**, which live on the light
                // rather than on a layer. Without them a live model that
                // keyed a lamp had no way to see the key had landed, and
                // spent its turns adding it, undoing it and adding it again.
                // `selected_light` is the one `add_light_keyframe` keys.
                "selected_light": editor.light_panel.selected.map(|id| id.0),
                "lights": scene
                    .lights()
                    .lights
                    .iter()
                    .map(|light| {
                        let keys: Vec<Value> = light
                            .track
                            .as_ref()
                            .map(|t| {
                                t.keys()
                                    .iter()
                                    .map(|k| json!({"frame": k.frame, "intensity": k.intensity}))
                                    .collect()
                            })
                            .unwrap_or_default();
                        json!({
                            "id": light.id.0,
                            "name": light.name,
                            "kind": format!("{:?}", light.kind),
                            "intensity": light.intensity,
                            "keys": keys,
                        })
                    })
                    .collect::<Vec<_>>(),
            })
        }
        // Everything on stage at the playhead, with kind and drawn bounds.
        "objects" => json!({"frame": editor.current_frame, "objects": objects_here(app)}),
        "selection" => json!({
            "objects": editor.selection.ids().iter().map(|id| id.0).collect::<Vec<_>>(),
            "active_layer": editor.selection.active_layer().map(|l| l.0),
        }),
        // `assets` is offered in the tool schema; the library answer lists both.
        "library" | "assets" => {
            let symbols: Vec<Value> = scene
                .library()
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id.0,
                        "name": s.name,
                        "kind": format!("{:?}", s.kind),
                        "folder": s.folder,
                    })
                })
                .collect();
            let assets: Vec<Value> = editor
                .assets
                .assets()
                .iter()
                .map(|a| json!({"name": a.name, "folder": a.folder}))
                .collect();
            json!({"symbols": symbols, "assets": assets})
        }
        "tools" => json!({
            "tool": format!("{:?}", editor.tool()),
            "style": format!("{:?}", editor.style),
            "auto_keyframe": editor.auto_keyframe,
        }),
        "catalog" => catalog_with(app.agent_allow_all()),
        other => return AgentResult::err(format!("unknown inspect target '{other}'")),
    };
    AgentResult::ok(value)
}

// -- Stage B: gestures and style -------------------------------------------

/// Lower-case, letters and digits only, so `paint_bucket`, `PaintBucket` and
/// `Paint Bucket` are the same name.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Find `name` among `all` by its `Debug` name, ignoring case and separators.
fn pick_named<T: Copy + std::fmt::Debug>(all: &[T], name: &str, what: &str) -> Result<T, String> {
    let want = norm(name);
    all.iter()
        .copied()
        .find(|v| norm(&format!("{v:?}")) == want)
        .ok_or_else(|| {
            let valid: Vec<String> = all.iter().map(|v| format!("{v:?}")).collect();
            format!("unknown {what} '{name}'; valid: {}", valid.join(", "))
        })
}

/// Read `#RGB`, `#RRGGBB` or `#RRGGBBAA`.
fn parse_hex(text: &str) -> Option<peniko::Color> {
    let text = text.trim().trim_start_matches('#');
    if !text.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(text.get(i..i + 2)?, 16).ok();
    match text.len() {
        3 => {
            let digit = |i: usize| u8::from_str_radix(text.get(i..i + 1)?, 16).ok();
            Some(peniko::Color::from_rgba8(digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255))
        }
        6 => Some(peniko::Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, 255)),
        8 => Some(peniko::Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

/// A colour as `#rrggbb`, or `#rrggbbaa` when it is not opaque.
fn hex_of(color: peniko::Color) -> String {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// Every object at the playhead, across layers: id, layer, kind and colours.
/// Capped so a busy scene cannot flood the model.
fn objects_here(app: &App) -> Vec<Value> {
    let editor = app.agent_editor();
    let frame = editor.current_frame;
    let mut out = Vec::new();
    for layer in editor.doc.scene().layers().iter() {
        for object in layer.objects_at(frame) {
            if out.len() >= 200 {
                return out;
            }
            let (kind, fill, stroke) = match &object.kind {
                buzz_scene::ObjectKind::Shape(s) => (
                    "shape",
                    s.fill.as_ref().map(|f| hex_of(f.paint.color())),
                    s.stroke.as_ref().map(|st| hex_of(st.paint.color())),
                ),
                buzz_scene::ObjectKind::Group(_) => ("group", None, None),
                buzz_scene::ObjectKind::Instance(_) => ("instance", None, None),
                buzz_scene::ObjectKind::Armature(_) => ("armature", None, None),
                buzz_scene::ObjectKind::Warp(_) => ("warp", None, None),
            };
            // **Where it is and how big**, in the document coordinates
            // `gesture` takes. Without it a model placing a character had no
            // way to know its size, so it could neither scale it to stand
            // beside another nor put its feet on the ground.
            let bounds = editor.focal_local_and_quad(object.id).map(|(_, quad)| {
                let r = quad.iter().skip(1).fold(kurbo::Rect::from_points(quad[0], quad[0]), |r, p| r.union_pt(*p));
                let t = |v: f64| (v * 10.0).round() / 10.0;
                json!([t(r.x0), t(r.y0), t(r.x1), t(r.y1)])
            });
            out.push(json!({
                "id": object.id.0,
                "layer": layer.id.0,
                "kind": kind,
                "fill": fill,
                "stroke": stroke,
                "bounds": bounds,
            }));
        }
    }
    out
}

fn selection_ids(app: &App) -> Vec<u64> {
    app.agent_editor().selection.ids().iter().map(|id| id.0).collect()
}

fn gesture(app: &mut App, args: &Value) -> AgentResult {
    let Some(name) = args.get("tool").and_then(|v| v.as_str()) else {
        return AgentResult::err("gesture needs a 'tool'");
    };
    let tool = match pick_named(&buzz_ui::all_tools(), name, "tool") {
        Ok(t) => t,
        Err(e) => return AgentResult::err(e),
    };
    let Some(raw) = args.get("points").and_then(|v| v.as_array()).filter(|p| !p.is_empty()) else {
        return AgentResult::err("gesture needs a non-empty 'points' array of {x,y,pressure?,t?}");
    };
    let mut points = Vec::with_capacity(raw.len());
    for (i, p) in raw.iter().enumerate() {
        let (Some(x), Some(y)) = (
            p.get("x").and_then(|v| v.as_f64()),
            p.get("y").and_then(|v| v.as_f64()),
        ) else {
            return AgentResult::err(format!("points[{i}] needs numeric x and y"));
        };
        if !x.is_finite() || !y.is_finite() {
            return AgentResult::err(format!("points[{i}] is not finite"));
        }
        points.push((
            buzz_geom::Point::new(x, y),
            p.get("t").and_then(|v| v.as_f64()),
            p.get("pressure").and_then(|v| v.as_f64()),
        ));
    }
    let flag = |key: &str| {
        args.get("mods")
            .and_then(|m| m.get(key))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    let mods = crate::tools::Mods {
        shift: flag("shift"),
        alt: flag("alt"),
        ctrl: flag("ctrl"),
    };

    if !tool.is_ready() {
        return AgentResult::err(format!("tool '{}' is not ready yet", tool.name()));
    }
    let objects_before = objects_here(app).len();
    let selection_before = selection_ids(app);
    let depth_before = app.agent_editor().doc.history().undo_depth();

    let editor = app.agent_editor_mut();
    editor.status = None;
    editor.set_tool(tool);
    if editor.tool() != tool {
        return AgentResult::err(format!("could not select tool '{}'", tool.name()));
    }
    // Points are in document (edit) space; the editor's pointer calls take
    // screen points, so go back through the same view the mouse sees.
    let to_screen = |editor: &crate::editor::Editor, p: buzz_geom::Point| {
        let stage = editor.edit_to_stage().map_point(p).unwrap_or(p);
        editor.camera.doc_to_screen(stage)
    };
    let (first, t0, p0) = points[0];
    let s = to_screen(editor, first);
    editor.pointer_down_at(s, mods, t0, p0);
    for &(p, t, pr) in &points[1..] {
        let s = to_screen(editor, p);
        editor.pointer_move_at(s, mods, t, pr);
    }
    let (last, tl, pl) = *points.last().unwrap();
    let s = to_screen(editor, last);
    editor.pointer_up_at(s, tl, pl);

    let objects = objects_here(app);
    let selection = selection_ids(app);
    let depth_after = app.agent_editor().doc.history().undo_depth();
    let mut value = report(app, "gesture");
    value["tool"] = json!(tool.name());
    value["object_count"] = json!(objects.len());
    value["object_delta"] = json!(objects.len() as i64 - objects_before as i64);
    value["objects"] = Value::Array(objects);
    value["selection_changed"] = json!(selection != selection_before);
    value["selection"] = json!(selection);
    value["undo_steps_added"] = json!(depth_after as i64 - depth_before as i64);
    // **Say when nothing happened, and the likely why.** A live model "drew"
    // its circle with one point, read `ok: true`, and carried on as if a ball
    // existed. A single point is a click; the shape tools draw on a drag.
    if depth_after == depth_before && objects_here_len(&value) == objects_before && selection == selection_before {
        value["note"] = json!(if points.len() == 1 {
            "nothing changed: one point is a click. Shape tools (rectangle, oval, line…) draw on a drag, so pass at least two points: start and end corner"
        } else {
            "nothing changed: check the tool, the layer (locked or hidden?) and that the points are on the stage"
        });
    }
    AgentResult::ok(value)
}

fn objects_here_len(value: &Value) -> usize {
    value["object_count"].as_u64().unwrap_or(0) as usize
}

pub(crate) const STYLE_FIELDS: &[&str] = &[
    "stroke",
    "fill",
    "stroke_enabled",
    "fill_enabled",
    "stroke_width",
    "brush_size",
    "brush_kind",
    "effect_brush",
    "brush_smoothing",
    "eraser_size",
    "eraser_mode",
    "hairline",
    "stroke_kind",
    "drawing_mode",
    "auto_keyframe",
];

fn apply_style_field(
    key: &str,
    v: &Value,
    style: &mut buzz_ui::DrawStyle,
    auto_keyframe: &mut bool,
) -> Result<(), String> {
    let colour = || {
        v.as_str()
            .and_then(parse_hex)
            .ok_or_else(|| format!("'{key}' needs a colour like \"#rrggbb\" or \"#rrggbbaa\""))
    };
    let number = || {
        v.as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or_else(|| format!("'{key}' needs a non-negative number"))
    };
    let boolean = || v.as_bool().ok_or_else(|| format!("'{key}' needs true or false"));
    let text = || v.as_str().ok_or_else(|| format!("'{key}' needs a name"));
    match key {
        "stroke" => {
            style.stroke_color = colour()?;
            style.stroke_enabled = true;
        }
        "fill" => {
            style.fill_color = colour()?;
            style.fill_enabled = true;
            style.fill_kind = buzz_ui::style::FillKind::Solid;
        }
        "stroke_enabled" => style.stroke_enabled = boolean()?,
        "fill_enabled" => style.fill_enabled = boolean()?,
        "stroke_width" => style.stroke_width = number()?,
        "brush_size" => style.brush.size = number()?,
        "brush_smoothing" => style.brush.smoothing = number()?.min(1.0),
        "eraser_size" => style.eraser_size = number()?,
        "hairline" => style.hairline = boolean()?,
        "auto_keyframe" => *auto_keyframe = boolean()?,
        "brush_kind" => style.brush.kind = pick_named(&buzz_ui::BrushKind::ALL, text()?, key)?,
        "effect_brush" => {
            style.brush.effect = pick_named(&buzz_scene::EffectKind::ALL, text()?, key)?
        }
        "eraser_mode" => {
            style.eraser_mode = pick_named(&buzz_ui::style::EraserMode::ALL, text()?, key)?
        }
        "stroke_kind" => {
            use buzz_ui::style::StrokeKind::*;
            style.stroke_kind = pick_named(&[Solid, Dashed, Dotted], text()?, key)?
        }
        "drawing_mode" => {
            use buzz_ui::style::DrawingMode::*;
            style.drawing_mode = pick_named(&[MergeShape, ObjectDrawing], text()?, key)?
        }
        _ => return Err(format!("unknown style field '{key}'")),
    }
    Ok(())
}

fn set_style(app: &mut App, args: &Value) -> AgentResult {
    let valid = STYLE_FIELDS.join(", ");
    let Some(map) = args.as_object().filter(|m| !m.is_empty()) else {
        return AgentResult::err(format!("set_style needs at least one field; valid: {valid}"));
    };
    // Check every name first, and apply to a copy, so a bad call changes nothing.
    for key in map.keys() {
        if !STYLE_FIELDS.contains(&key.as_str()) {
            return AgentResult::err(format!("unknown style field '{key}'; valid: {valid}"));
        }
    }
    let editor = app.agent_editor_mut();
    let mut style = editor.style.clone();
    let mut auto_keyframe = editor.auto_keyframe;
    for (key, v) in map {
        if let Err(e) = apply_style_field(key, v, &mut style, &mut auto_keyframe) {
            return AgentResult::err(e);
        }
    }
    editor.style = style;
    editor.auto_keyframe = auto_keyframe;
    let s = &editor.style;
    AgentResult::ok(json!({
        "stroke": hex_of(s.stroke_color),
        "fill": hex_of(s.fill_color),
        "stroke_enabled": s.stroke_enabled,
        "fill_enabled": s.fill_enabled,
        "stroke_width": s.stroke_width,
        "brush_size": s.brush.size,
        "brush_kind": format!("{:?}", s.brush.kind),
        "effect_brush": format!("{:?}", s.brush.effect),
        "auto_keyframe": editor.auto_keyframe,
    }))
}

// -- Stage C: script and snapshot ------------------------------------------

/// Run JSFL-style JavaScript on this thread, the way `Editor::run_script`
/// does: same context, default limits, one undo step if the document changed.
fn script(app: &mut App, args: &Value) -> AgentResult {
    let Some(code) = args.get("code").and_then(|v| v.as_str()) else {
        return AgentResult::err("script needs 'code'");
    };
    let stop_flag = app.agent_stop_flag();
    let editor = app.agent_editor_mut();
    let context = buzz_script::ScriptContext {
        current_frame: editor.current_frame,
        selection: editor.selection.ids(),
        active_layer: editor.selection.active_layer(),
        config_dir: buzz_script::default_config_dir(),
        asset_root: Some(buzz_doc::assets::default_root()),
        prompt_answers: Vec::new(),
    };
    let mut working = editor.doc.scene().clone();
    // Stop reaches the interpreter between bytecodes, like the Script panel's.
    let stop: Option<buzz_script::StopSignal> = stop_flag.map(|flag| {
        let signal: buzz_script::StopSignal =
            std::sync::Arc::new(move || flag.load(std::sync::atomic::Ordering::Relaxed));
        signal
    });
    let outcome = buzz_script::run_until(&mut working, context, code, &buzz_script::Limits::default(), stop);
    if outcome.changed {
        editor.doc.edit("Run Script", |scene| *scene = working);
        editor.doc.end_gesture();
    }
    let summary = outcome.summary();
    editor.set_frame(outcome.context.current_frame);
    editor.selection.set(outcome.context.selection.clone());
    editor.selection.prune(editor.doc.scene());
    if let Some(layer) = outcome.context.active_layer {
        editor.selection.set_active_layer(Some(layer));
    }
    editor.selection.ensure_active_layer(editor.doc.scene());
    editor.status = Some(summary.clone());

    let mut value = report(app, "script");
    value["trace"] = json!(outcome.trace);
    value["alerts"] = json!(outcome.alerts);
    value["changed"] = json!(outcome.changed);
    value["summary"] = json!(summary);
    AgentResult {
        ok: outcome.error.is_none(),
        value,
        // **An error a model can act on.** The interpreter says only "not a
        // function" with a column number; a live model then repeated the
        // same `fl.getDocument()` for its whole turn budget. The entry points
        // it needed go back with the error.
        error: outcome.error.map(|e| format!("{e}\n{JSFL_PRIMER}")),
    }
}

/// The JSFL entry points, sent back with a script error.
const JSFL_PRIMER: &str = "JSFL here: var doc = fl.getDocumentDOM(); var tl = doc.getTimeline(); \
tl.layers[i] (name, frames[f].elements); tl.currentFrame = n; tl.addNewLayer(name); \
tl.insertKeyframe(f); doc.setTween(layerIndex, keyframe, \"classic\"|\"motion\"|\"shape\"|\"none\"); \
tl.layers[i].frames[f].tweenType = \"classic\"; \
doc.addNewOval({left,top,right,bottom}); doc.addNewRectangle({left,top,right,bottom}); \
doc.selection; doc.setFillColor(\"#rrggbb\"); fl.trace(x). \
Often a direct tool is simpler: command, gesture, set_style, set_property.";

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The longest side of a snapshot, in pixels.
const SNAPSHOT_MAX_SIDE: f64 = 768.0;

/// A PNG of the stage at the playhead (or `frame`), through the exporter, so
/// it is what an Export Image would hold. Errors starting `snapshot unavailable`
/// mean no GPU could be brought up.
fn snapshot(app: &App, args: &Value) -> AgentResult {
    let editor = app.agent_editor();
    let scene = editor.doc.scene();
    let frame = match args.get("frame") {
        None | Some(Value::Null) => editor.current_frame,
        Some(v) => match v.as_u64().and_then(|f| u32::try_from(f).ok()) {
            Some(f) => f,
            None => return AgentResult::err("'frame' must be a non-negative integer"),
        },
    };
    let full = buzz_export::ExportSettings::for_stage(scene);
    let longest = full.width.max(full.height) as f64;
    let settings = buzz_export::ExportSettings::scaled(scene, (SNAPSHOT_MAX_SIDE / longest).min(1.0));
    let mut exporter = match buzz_export::Exporter::new(&buzz_render::GpuPreference::default()) {
        Ok(e) => e,
        Err(e) => return AgentResult::err(format!("snapshot unavailable: {e}")),
    };
    let rendered = match exporter.render(scene, frame, &settings) {
        Ok(r) => r,
        Err(e) => return AgentResult::err(format!("snapshot unavailable: {e}")),
    };
    match rendered.to_png() {
        Ok(png) => AgentResult::ok(json!({
            "frame": frame,
            "width": rendered.width,
            "height": rendered.height,
            "png_base64": base64(&png),
        })),
        Err(e) => AgentResult::err(format!("snapshot failed: {e}")),
    }
}

/// The tools beyond `command` and `inspect`, with their argument hints.
pub const AGENT_TOOLS: &[(&str, &str)] = &[
    ("gesture", "{tool: tool id e.g. rectangle|brush|paint_bucket|selection, points: [{x,y,pressure?,t?}] in document coords (one point = click), mods?: {shift,alt,ctrl}}"),
    ("set_style", "{stroke?: \"#rrggbb\", fill?: \"#rrggbb\", stroke_width?, brush_size?, brush_kind?, effect_brush?, brush_smoothing?, eraser_size?, eraser_mode?, hairline?, stroke_kind?, drawing_mode?, stroke_enabled?, fill_enabled?, auto_keyframe?}"),
    ("script", "{code: JSFL JavaScript, one undo step; returns fl.trace output and errors}"),
    ("snapshot", "{frame?: 0-based frame} -> {png_base64, width, height}, longest side <= 768"),
    ("set_property", "{target: element:<id>|layer:<index|name> (index = inspect timeline \"index\", not \"id\")|frame:<layer>:<frame>|light:<id>|light:rig|camera|document|scene, field, value}; inspect {what:\"properties\", target} lists the fields, values and hints. {target:\"document\", field:\"playhead\", value:<0-based frame>} moves the playhead (frame 24 = 23)"),
    ("edit_list", "{target: element:<id>|layer:<index|name> (index = inspect timeline \"index\", not \"id\")|document, list: filters|modifiers|swatches|swatch_folders|poses|bones|rig, op: get|add|remove|move|set (rig list: set_rest_pose|mirror_pose|reset_pose), index?, to?, item?}; one undo step per op, returns the list. filters (element|layer): item {kind: blur|drop_shadow|glow|bevel|adjust|gradient_map, enabled?, <kind parameters>}, move uses to. modifiers (element): item {kind: wiggle|spring|look_at|squash_stretch|breathe|blink|turn|sway|drift, <parameters>}. swatches (document): add {name, color, folder?}; set index {name?, color? (repaints linked fills), folder?|null}; remove; move index to folder. swatch_folders (document): add {name}; remove {name}|index. poses (armature element): add {name} saves the pose; set index applies (item {key:true} also keys it); remove index. bones (armature element): remove index deletes a bone; move index to parent bone|null reparents"),
    ("wait_for_jobs", "{timeout_s?: 1..300 (default 120)} -> {jobs:[{id, kind, status: running|done|failed, result?, error?}], running, timed_out}; waits for every AI job started in this run (ai_rig_vector_character, ai_direct_story, ai_character_from_image, ai_scene_from_image, transcribe) and returns as soon as none is running, or at the timeout with the ones still running; the BuzzEdit and Studio tools' jobs are waited for the same way. A finished job's result is given once"),
    ("transcribe", "{sound?: name of a sound in the library, path?: audio file} (exactly one) -> {started:true, job}; Whisper runs in the background (the multilingual model when installed, else base.en); call wait_for_jobs for {text, language, model, timing, speakers:\"not identified\", segments:[{start_s, end_s, text}]}. There is no speaker separation: infer who speaks from the content"),
    ("send_to_buzzedit", "{scope?: document|scene (default document: the whole film; scene: the scene being edited), project?: BuzzEdit project id from buzzedit_projects (default: a new BuzzEdit project), path?: .mp4 to write (must not exist; default beside the document), start?: bool (launch BuzzEdit if it is not running)} -> {started:true, job}; exports an MP4 on the export queue, then imports it into BuzzEdit; wait_for_jobs gives {project_id, project_name, new_project, video}. Needs ffmpeg"),
    ("buzzedit_projects", "{start?: bool} -> {started:true, job}; wait_for_jobs gives {projects:[{id, name, status}]} from BuzzEdit"),
    ("transcribe_with_buzzedit", "{sound: name of a sound in the library, speaker: who is talking (names the mouth symbol to lip-sync), start?: bool} -> {started:true, job}; BuzzEdit's Whisper hears the sound (it becomes a BuzzEdit project), its words become captions on a Captions layer and the speaker's lip sync; wait_for_jobs gives {buzzedit_project, words, captions, lip_sync_keys, status}"),
    ("studio_projects", "{start?: bool} -> {started:true, job}; wait_for_jobs gives {projects:[{id, name, brand, status, current_step, has_script}]} from Buzzcaf Studio"),
    ("studio_build_episode", "{project: Studio project id, asset?: the project's script asset (default script), out_dir?: folder (default <the Studio project's folder>/animation), start?: bool} -> {started:true, job}; fetches the script and builds the episode (.buzz + report.md) as POST /api/episode does; wait_for_jobs gives {files, warnings, out_dir}. The script needs '## SCENE n — TITLE' headings"),
    ("dress_set_from_assets", "{query: the scene's words (slugline + what happens, e.g. \"EXT. CHHOTU'S TEA STALL - MORNING, a village lane\"; INT./room/inside -> a room; night/evening -> that sky), style?: name of an asset whose style to follow} -> dresses the open scene's set from the user's own assets (the asset index): a sky, trees and buildings along the horizon at three depths, bushes clear of the rigs on stage, or one room indoors; behind everything already there (in front of Set the scene's Sky/Ground layers; with none, a ground and a sky are staged first); one undo step. Returns {pieces:[{role, name, record, why}], notes, failed, interior, procedural_advised, layers}. Needs the index"),
    ("arrange_assets", "{dry_run?: bool (default true)} -> Arrange by type & style from the asset index: dry run gives {moves, counts:{\"Type/Style\": n}, cast_kept, unindexed, first_moves:[{name, from, to, guessed}]} and moves nothing; dry_run:false moves the files (Cast/ never) and returns {moved, created_folders}; the Assets panel's Undo arrange reverses it. Needs the index (built by the Assets panel's Arrange by type & style…)"),
];

/// The tool catalog the model sees, generated from code so it cannot drift:
/// one `{id, label, args, how}` per command.
pub fn catalog() -> Value {
    catalog_with(false)
}

/// [`catalog`], marking the excluded commands "allowed (Allow all)" when that
/// switch is on (quit and close stay excluded).
pub fn catalog_with(allow_all: bool) -> Value {
    let describe = |route: Route| match route {
        Route::Run => "run".to_string(),
        Route::Path(_) => "path".to_string(),
        Route::NeedsDialog => "needs_dialog".to_string(),
        Route::Args => "args".to_string(),
        Route::Save => "run_if_saved".to_string(),
        Route::Excluded => "excluded".to_string(),
    };
    let describe_cmd = |c: Command, route: Route| {
        if allow_all && matches!(route, Route::Excluded) && allow_all_unlocks(c.id()) {
            "allowed (Allow all)".to_string()
        } else {
            describe(route)
        }
    };
    let mut entries: Vec<Value> = Command::unit_commands()
        .iter()
        .map(|&c| {
            let route = route(c);
            let args = if matches!(route, Route::Path(_)) {
                "{path: file path}"
            } else if matches!(route, Route::Args) {
                dialogs::args_hint(c).unwrap_or("")
            } else {
                ""
            };
            json!({"id": c.id(), "label": c.label(), "args": args, "how": describe_cmd(c, route)})
        })
        .collect();
    for (id, hint) in Command::data_command_hints() {
        entries.push(json!({"id": id, "label": id, "args": hint, "how": "run"}));
    }
    for (id, hint) in AGENT_TOOLS {
        entries.push(json!({"id": id, "label": id, "args": hint, "how": "tool"}));
    }
    Value::Array(entries)
}
