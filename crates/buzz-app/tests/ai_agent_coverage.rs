//! Stage A of `docs/AI_AGENT_PLAN.md`: the AI agent's executor reaches every
//! editor command by id, headlessly, or says why it cannot.

use buzz_app::agent::{AGENT_EXCLUDED, AgentCall, Route, execute, route};
use buzz_app::app::App;
use buzz_render::GpuPreference;
use buzz_ui::Command;
use serde_json::{Value, json};

fn call(app: &mut App, tool: &str, args: Value) -> buzz_app::agent::AgentResult {
    execute(
        app,
        &AgentCall {
            tool: tool.to_string(),
            args,
        },
    )
}

fn cmd(app: &mut App, name: &str) -> buzz_app::agent::AgentResult {
    call(app, "command", json!({"name": name}))
}

fn layer_count(app: &mut App) -> usize {
    let r = call(app, "inspect", json!({"what": "timeline"}));
    assert!(r.ok, "{:?}", r.error);
    r.value["layers"].as_array().unwrap().len()
}

/// One sample of every data-carrying command, with the args that build it.
fn data_samples() -> Vec<(&'static str, Value)> {
    vec![
        ("toggle_panel", json!({"panel": "layers"})),
        ("set_theme", json!({"theme": "dark"})),
        ("set_ui_scale", json!({"scale": 100})),
        ("set_workspace_preset", json!({"name": "animator"})),
        ("set_master_audio_from_layer", json!({"layer": 1})),
        ("new_shot_from_setup", json!({"setup": 0})),
        ("try_another_setup", json!({"shot": 0, "setup": 1})),
        ("duplicate_shot", json!({"shot": 0})),
        ("add_missing_setup_layers", json!({"shot": 0})),
        ("edit_setup", json!({"setup": 0})),
        ("cut_on_dialogue", json!({"setup": 0})),
        ("continue_from_shot", json!({"shot": 1, "source": 0})),
        ("add_camera_move", json!({"move": "push_in"})),
        ("select_tool", json!({"tool": "brush"})),
        ("new_from_template", json!({"index": 0})),
        ("align", json!({"edge": "left_edges", "to_stage": true})),
        ("distribute", json!({"mode": "horizontal_spacing"})),
        ("match_size", json!({"mode": "both"})),
        ("nudge", json!({"dx": 1, "dy": 0})),
        ("move_frames", json!({"to": 3, "rows": 1, "copy": false})),
    ]
}

#[test]
fn every_command_id_round_trips() {
    for &c in Command::unit_commands() {
        assert_eq!(Command::from_id(c.id(), &Value::Null), Ok(c), "{}", c.id());
    }
    let samples = data_samples();
    assert_eq!(samples.len(), Command::data_command_hints().len());
    for (id, args) in samples {
        let c = Command::from_id(id, &args).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(c.id(), id);
    }
}

#[test]
fn every_command_is_executable_or_excluded_with_a_reason() {
    let mut excluded_seen = Vec::new();
    for &c in Command::unit_commands() {
        let listed = AGENT_EXCLUDED.iter().any(|(id, _)| *id == c.id());
        match route(c) {
            Route::Excluded => {
                assert!(listed, "{} is routed Excluded but not in AGENT_EXCLUDED", c.id());
                excluded_seen.push(c.id());
            }
            _ => assert!(!listed, "{} is in AGENT_EXCLUDED but routed as executable", c.id()),
        }
    }
    for (id, reason) in AGENT_EXCLUDED {
        assert!(!reason.is_empty());
        assert!(excluded_seen.contains(id), "AGENT_EXCLUDED lists unknown/unrouted '{id}'");
    }
    // Data-carrying commands are all executable.
    for (id, args) in data_samples() {
        let c = Command::from_id(id, &args).unwrap();
        assert_eq!(route(c), Route::Run, "{id}");
    }
}

#[test]
fn file_dialog_and_modal_commands_never_open_a_dialog() {
    let mut app = App::new(GpuPreference::default());
    for &c in Command::unit_commands() {
        match route(c) {
            Route::Path(_) => {
                let r = cmd(&mut app, c.id());
                assert!(!r.ok, "{} ran without a path", c.id());
                assert!(r.error.unwrap().starts_with(&format!("needs_dialog {}", c.id())));
            }
            Route::NeedsDialog => {
                let r = cmd(&mut app, c.id());
                assert!(r.error.unwrap().starts_with(&format!("needs_dialog {}", c.id())));
            }
            Route::Excluded => {
                let r = cmd(&mut app, c.id());
                assert!(!r.ok && r.error.unwrap().starts_with("excluded"), "{}", c.id());
            }
            _ => {}
        }
    }
}

#[test]
fn a_path_command_runs_the_pick_handler() {
    let mut app = App::new(GpuPreference::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.buzz");
    let r = call(
        &mut app,
        "command",
        json!({"name": "save_as", "path": path.to_str().unwrap()}),
    );
    assert!(r.ok, "{:?}", r.error);
    assert!(path.exists(), "save_as with a path wrote the file");
    let doc = call(&mut app, "inspect", json!({"what": "document"}));
    assert_eq!(doc.value["path"], json!(path.display().to_string()));
}

/// Found by the live run (2026-10-05): a model read the new layer's `id` from
/// `inspect timeline`, passed it to `set_property layer:<n>` (which counts by
/// position) and renamed the wrong layer; it had no way to reach frame 24 but
/// stepping; and `inspect assets`, offered in the schema, was refused.
#[test]
fn layer_index_playhead_and_assets_line_up_for_a_model() {
    let mut app = App::new(GpuPreference::default());
    assert!(cmd(&mut app, "new_layer").ok);
    let t = call(&mut app, "inspect", json!({"what": "timeline"}));
    let index = t.value["active_layer_index"].as_u64().expect("active_layer_index");
    let layer = &t.value["layers"][index as usize];
    assert_eq!(layer["index"], json!(index));
    assert_eq!(layer["id"], t.value["active_layer"]);

    let r = call(
        &mut app,
        "set_property",
        json!({"target": format!("layer:{index}"), "field": "name", "value": "Ball"}),
    );
    assert!(r.ok, "{:?}", r.error);
    let t = call(&mut app, "inspect", json!({"what": "timeline"}));
    let active = t.value["active_layer"].clone();
    let renamed = t.value["layers"].as_array().unwrap().iter().find(|l| l["id"] == active).unwrap();
    assert_eq!(renamed["name"], "Ball", "the index named a different layer than the active one");

    let r = call(&mut app, "set_property", json!({"target": "document", "field": "playhead", "value": 23}));
    assert!(r.ok, "{:?}", r.error);
    let t = call(&mut app, "inspect", json!({"what": "timeline"}));
    assert_eq!(t.value["playhead"], json!(23));
    let r = call(&mut app, "set_property", json!({"target": "document", "field": "playhead", "value": 999999}));
    assert!(!r.ok, "a frame past the end must be refused");

    assert!(call(&mut app, "inspect", json!({"what": "assets"})).ok);

    // A light key a model sets has to be visible to it afterwards.
    assert!(cmd(&mut app, "add_lamp").ok);
    let r = cmd(&mut app, "add_light_keyframe");
    assert!(r.ok, "{:?}", r.error);
    let t = call(&mut app, "inspect", json!({"what": "timeline"}));
    let selected = t.value["selected_light"].clone();
    assert!(!selected.is_null(), "adding a lamp should select it: {}", t.value);
    let lamp = t.value["lights"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == selected)
        .expect("the selected light is listed");
    let frames: Vec<u64> = lamp["keys"].as_array().unwrap().iter().map(|k| k["frame"].as_u64().unwrap()).collect();
    assert_eq!(frames, vec![23], "the key should be at the playhead");
}

/// A model placing a character has to know where things are and how big:
/// `inspect objects` lists everything on stage with its drawn bounds, in the
/// same document coordinates `gesture` takes.
#[test]
fn inspect_objects_reports_drawn_bounds_in_gesture_coordinates() {
    let mut app = App::new(GpuPreference::default());
    let r = call(&mut app, "gesture", json!({"tool": "rectangle", "points": [{"x": 100, "y": 200}, {"x": 300, "y": 260}]}));
    assert!(r.ok, "{:?}", r.error);
    let r = call(&mut app, "inspect", json!({"what": "objects"}));
    assert!(r.ok, "{:?}", r.error);
    let objects = r.value["objects"].as_array().expect("objects");
    let b = objects
        .iter()
        .filter_map(|o| o["bounds"].as_array())
        .find(|b| b.len() == 4)
        .expect("a drawn object with bounds");
    let v: Vec<f64> = b.iter().map(|n| n.as_f64().unwrap()).collect();
    // The fill's box, give or take the outline's half-width.
    assert!((v[0] - 100.0).abs() < 6.0 && (v[1] - 200.0).abs() < 6.0, "{v:?}");
    assert!((v[2] - 300.0).abs() < 6.0 && (v[3] - 260.0).abs() < 6.0, "{v:?}");
}

/// A one-point oval is a click that draws nothing; the result must say so,
/// or a model carries on as if the shape exists (seen live, 2026-10-05).
#[test]
fn a_gesture_that_changes_nothing_says_why() {
    let mut app = App::new(GpuPreference::default());
    let r = call(&mut app, "gesture", json!({"tool": "oval", "points": [{"x": 300, "y": 300}]}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["object_delta"], json!(0));
    assert!(r.value["note"].as_str().is_some_and(|n| n.contains("two points")), "{}", r.value);

    let r = call(&mut app, "gesture", json!({"tool": "oval", "points": [{"x": 300, "y": 300}, {"x": 400, "y": 380}]}));
    assert!(r.ok, "{:?}", r.error);
    assert!(r.value["object_delta"].as_i64().unwrap_or(0) > 0, "{}", r.value);
    assert!(r.value.get("note").is_none(), "a drag that drew must carry no note: {}", r.value);
}

#[test]
fn headless_app_runs_commands_and_inspect_reflects_them() {
    let mut app = App::new(GpuPreference::default());
    let before = layer_count(&mut app);

    assert!(cmd(&mut app, "new_layer").ok);
    assert_eq!(layer_count(&mut app), before + 1);

    // A keyframe one frame along, on the new layer.
    assert!(cmd(&mut app, "next_frame").ok);
    assert!(cmd(&mut app, "insert_keyframe").ok);
    let t = call(&mut app, "inspect", json!({"what": "timeline"}));
    assert_eq!(t.value["playhead"], json!(1));
    let active = t.value["active_layer"].clone();
    let layer = t.value["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == active)
        .expect("active layer listed");
    let starts: Vec<u64> = layer["keyframes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["start"].as_u64().unwrap())
        .collect();
    assert!(starts.contains(&1), "keyframe at frame 1 present: {starts:?}");

    // Undo takes the keyframe back, then the layer.
    assert!(cmd(&mut app, "undo").ok);
    let t = call(&mut app, "inspect", json!({"what": "timeline"}));
    let layer = t.value["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == active)
        .unwrap();
    assert_eq!(layer["keyframes"].as_array().unwrap().len(), 1);
    assert!(cmd(&mut app, "undo").ok);
    assert_eq!(layer_count(&mut app), before);

    // A data-carrying command, by args.
    let r = call(&mut app, "command", json!({"name": "select_tool", "args": {"tool": "oval"}}));
    assert!(r.ok, "{:?}", r.error);
    let tools = call(&mut app, "inspect", json!({"what": "tools"}));
    assert_eq!(tools.value["tool"], json!("Oval"));
}

#[test]
fn inspect_answers_every_target_and_the_catalog_lists_every_command() {
    let mut app = App::new(GpuPreference::default());
    for what in ["document", "timeline", "selection", "library", "tools", "catalog"] {
        let r = call(&mut app, "inspect", json!({"what": what}));
        assert!(r.ok, "{what}: {:?}", r.error);
    }
    let cat = call(&mut app, "inspect", json!({"what": "catalog"}));
    assert_eq!(
        cat.value.as_array().unwrap().len(),
        Command::unit_commands().len()
            + Command::data_command_hints().len()
            + buzz_app::agent::AGENT_TOOLS.len()
    );
}

#[test]
fn bad_calls_are_results_not_panics() {
    let mut app = App::new(GpuPreference::default());
    let r = cmd(&mut app, "no_such_command");
    assert!(!r.ok && r.error.is_some());
    assert!(!call(&mut app, "command", json!({})).ok);
    assert!(!call(&mut app, "command", json!({"name": "select_tool"})).ok);
    assert!(!call(&mut app, "inspect", json!({"what": "nope"})).ok);
    assert!(!call(&mut app, "teleport", json!({})).ok);
}

// -- Stages B and C: gestures, style, script, snapshot ----------------------

use buzz_ui::ToolId;

/// How the agent reaches each tool. An exhaustive `match`: a new `ToolId`
/// fails to compile here until it is classified.
#[derive(Debug, PartialEq)]
enum ToolReach {
    /// Pressed, moved and released through `gesture` like the mouse.
    Gesture,
}

fn reach(tool: ToolId) -> ToolReach {
    use ToolId::*;
    match tool {
        Selection | Subselection | Lasso | MagicWand | FreeTransform | GradientTransform | Pen
        | Text | Line | Rectangle | Oval | PolyStar | Pencil | Brush | Bone | AssetWarp
        | MotionPath | PaintBucket | InkBottle | Eyedropper | Eraser | Camera | Hand | Zoom => {
            ToolReach::Gesture
        }
    }
}

fn pts(list: &[(f64, f64)]) -> Value {
    Value::Array(list.iter().map(|(x, y)| json!({"x": x, "y": y})).collect())
}

fn drag(app: &mut App, tool: &str, from: (f64, f64), to: (f64, f64)) -> buzz_app::agent::AgentResult {
    let mid = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
    call(app, "gesture", json!({"tool": tool, "points": pts(&[from, mid, to])}))
}

/// Every object at the playhead, read through a harmless Hand-tool click.
fn objects(app: &mut App) -> Vec<Value> {
    let r = call(app, "gesture", json!({"tool": "hand", "points": pts(&[(0.0, 0.0)])}));
    assert!(r.ok, "{:?}", r.error);
    r.value["objects"].as_array().unwrap().clone()
}

fn undo_depth(app: &mut App) -> u64 {
    call(app, "inspect", json!({"what": "document"})).value["undo_depth"].as_u64().unwrap()
}

#[test]
fn every_tool_is_reachable_through_gesture() {
    let mut partial: Vec<(String, String)> = Vec::new();
    for tool in buzz_ui::all_tools() {
        assert_eq!(reach(tool), ToolReach::Gesture);
        let id = format!("{tool:?}");
        let mut app = App::new(GpuPreference::default());
        let r = drag(&mut app, &id, (100.0, 100.0), (220.0, 180.0));
        if !r.ok {
            assert!(!tool.is_ready(), "{id}: {:?}", r.error);
            partial.push((id.clone(), r.error.unwrap()));
            continue;
        }
        let click = call(&mut app, "gesture", json!({"tool": id, "points": pts(&[(150.0, 150.0)])}));
        assert!(click.ok, "{id} click: {:?}", click.error);
        let t = call(&mut app, "inspect", json!({"what": "tools"}));
        assert_eq!(t.value["tool"], json!(id.as_str()), "{id} became the current tool");
    }
    eprintln!("tools unreachable or partial: {partial:?}");
    assert!(partial.is_empty(), "{partial:?}");
}

#[test]
fn gesture_bad_calls_are_results() {
    let mut app = App::new(GpuPreference::default());
    assert!(!call(&mut app, "gesture", json!({"points": pts(&[(1.0, 1.0)])})).ok);
    assert!(!call(&mut app, "gesture", json!({"tool": "nope", "points": pts(&[(1.0, 1.0)])})).ok);
    assert!(!call(&mut app, "gesture", json!({"tool": "brush", "points": []})).ok);
    assert!(!call(&mut app, "gesture", json!({"tool": "brush"})).ok);
    assert!(!call(&mut app, "gesture", json!({"tool": "brush", "points": [{"x": 1}]})).ok);
}

#[test]
fn set_style_rejects_unknown_fields_and_shows_in_inspect_tools() {
    let mut app = App::new(GpuPreference::default());
    let bad = call(&mut app, "set_style", json!({"colour": "#ff0000"}));
    assert!(!bad.ok);
    assert!(bad.error.unwrap().contains("fill"), "lists valid fields");
    assert!(!call(&mut app, "set_style", json!({"fill": "red"})).ok);
    assert!(!call(&mut app, "set_style", json!({"brush_kind": "nope"})).ok);

    let r = call(
        &mut app,
        "set_style",
        json!({"fill": "#336699", "stroke": "#ff000080", "stroke_width": 7, "brush_size": 22, "brush_kind": "normal", "effect_brush": "snow"}),
    );
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["fill"], json!("#336699"));
    let tools = call(&mut app, "inspect", json!({"what": "tools"}));
    let style = tools.value["style"].as_str().unwrap();
    assert!(style.contains("Normal") && style.contains("22"), "{style}");
}

#[test]
fn a_rectangle_drag_creates_a_shape_of_the_set_fill() {
    let mut app = App::new(GpuPreference::default());
    assert!(call(&mut app, "set_style", json!({"fill": "#ff0000"})).ok);
    let before = objects(&mut app).len();
    let r = drag(&mut app, "rectangle", (100.0, 100.0), (300.0, 250.0));
    assert!(r.ok, "{:?}", r.error);
    let after = objects(&mut app);
    assert!(after.len() > before, "a shape was added: {after:?}");
    assert!(after.iter().any(|o| o["fill"] == json!("#ff0000")), "{after:?}");
    assert!(r.value["undo_steps_added"].as_i64().unwrap() >= 1);
}

#[test]
fn a_brush_stroke_adds_a_shape() {
    let mut app = App::new(GpuPreference::default());
    assert!(call(&mut app, "set_style", json!({"stroke": "#00aa00", "fill": "#00aa00", "brush_size": 12})).ok);
    let before = objects(&mut app).len();
    let line: Vec<(f64, f64)> = (0..20)
        .map(|i| (100.0 + i as f64 * 10.0, 100.0 + (i as f64 * 0.5).sin() * 20.0))
        .collect();
    let r = call(&mut app, "gesture", json!({"tool": "brush", "points": pts(&line)}));
    assert!(r.ok, "{:?}", r.error);
    assert!(objects(&mut app).len() > before, "{:?}", r.value);
}

#[test]
fn the_paint_bucket_recolours_a_closed_shape() {
    let mut app = App::new(GpuPreference::default());
    assert!(call(&mut app, "set_style", json!({"fill": "#ff0000"})).ok);
    assert!(drag(&mut app, "rectangle", (100.0, 100.0), (300.0, 250.0)).ok);
    let count = objects(&mut app).len();
    assert!(call(&mut app, "set_style", json!({"fill": "#0000ff"})).ok);
    let r = call(&mut app, "gesture", json!({"tool": "paint_bucket", "points": pts(&[(200.0, 175.0)])}));
    assert!(r.ok, "{:?}", r.error);
    let after = objects(&mut app);
    assert_eq!(after.len(), count, "no new shapes: {after:?}");
    assert!(after.iter().any(|o| o["fill"] == json!("#0000ff")), "{after:?}");
    assert!(!after.iter().any(|o| o["fill"] == json!("#ff0000")), "{after:?}");
}

#[test]
fn a_selection_drag_marquee_selects() {
    let mut app = App::new(GpuPreference::default());
    assert!(drag(&mut app, "rectangle", (100.0, 100.0), (300.0, 250.0)).ok);
    assert!(call(&mut app, "command", json!({"name": "deselect"})).ok);
    let none = call(&mut app, "inspect", json!({"what": "selection"}));
    assert!(none.value["objects"].as_array().unwrap().is_empty());
    let r = call(
        &mut app,
        "gesture",
        json!({"tool": "selection", "points": pts(&[(50.0, 50.0), (200.0, 150.0), (350.0, 300.0)])}),
    );
    assert!(r.ok, "{:?}", r.error);
    assert!(r.value["selection_changed"].as_bool().unwrap(), "{:?}", r.value);
    let sel = call(&mut app, "inspect", json!({"what": "selection"}));
    assert!(!sel.value["objects"].as_array().unwrap().is_empty());
}

#[test]
fn a_script_adds_a_shape_in_one_undo_step() {
    let mut app = App::new(GpuPreference::default());
    let before = objects(&mut app).len();
    let depth = undo_depth(&mut app);
    let r = call(
        &mut app,
        "script",
        json!({"code": "var d = fl.getDocumentDOM(); d.addNewRectangle({left:10,top:10,right:110,bottom:90},0); fl.trace('made');"}),
    );
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["trace"], json!(["made"]));
    assert_eq!(objects(&mut app).len(), before + 1);
    assert_eq!(undo_depth(&mut app), depth + 1, "one undo step");
    assert!(cmd(&mut app, "undo").ok);
    assert_eq!(objects(&mut app).len(), before, "undo removed it");
    assert_eq!(undo_depth(&mut app), depth);
}

#[test]
fn script_errors_are_results_and_an_endless_loop_times_out() {
    let mut app = App::new(GpuPreference::default());
    let r = call(&mut app, "script", json!({"code": "this is not javascript"}));
    assert!(!r.ok && r.error.is_some());
    let started = std::time::Instant::now();
    let r = call(&mut app, "script", json!({"code": "while(true){}"}));
    assert!(!r.ok && r.error.is_some(), "{r:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(60));
    assert!(!call(&mut app, "script", json!({})).ok);
}

#[test]
fn snapshot_returns_a_decodable_png_or_skips_without_a_gpu() {
    let mut app = App::new(GpuPreference::default());
    assert!(call(&mut app, "set_style", json!({"fill": "#ff0000"})).ok);
    assert!(drag(&mut app, "rectangle", (100.0, 100.0), (300.0, 250.0)).ok);
    let r = call(&mut app, "snapshot", json!({}));
    if !r.ok {
        let e = r.error.unwrap();
        assert!(e.starts_with("snapshot unavailable"), "{e}");
        eprintln!("SKIPPED snapshot render: no GPU in this environment ({e})");
        return;
    }
    let (w, h) = (r.value["width"].as_u64().unwrap(), r.value["height"].as_u64().unwrap());
    assert!(w.max(h) <= 768, "{w}x{h}");
    let bytes = decode_base64(r.value["png_base64"].as_str().unwrap());
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("a PNG header");
    assert_eq!((reader.info().width as u64, reader.info().height as u64), (w, h));
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buf).expect("PNG pixels decode");
    assert!(!call(&mut app, "snapshot", json!({"frame": -1})).ok);
}

fn decode_base64(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64"),
    };
    let mut out = Vec::new();
    for chunk in text.as_bytes().chunks(4) {
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        let n = chunk
            .iter()
            .filter(|&&c| c != b'=')
            .fold(0u32, |n, &c| n << 6 | value(c) as u32)
            << (6 * pad);
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    out
}

// -- Stage D: set_property -------------------------------------------------

use buzz_app::agent::props::{PROPERTY_EXCLUDED, PROPERTY_FIELDS};

fn props(app: &mut App, target: &str) -> buzz_app::agent::AgentResult {
    call(app, "inspect", json!({"what": "properties", "target": target}))
}

fn set(app: &mut App, target: &str, field: &str, value: Value) -> buzz_app::agent::AgentResult {
    call(app, "set_property", json!({"target": target, "field": field, "value": value}))
}

/// The current value of one field, through `inspect properties`.
fn read(app: &mut App, target: &str, field: &str) -> Option<Value> {
    let r = props(app, target);
    assert!(r.ok, "{target}: {:?}", r.error);
    r.value["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["field"] == json!(field))
        .map(|f| f["value"].clone())
}

fn field_names(app: &mut App, target: &str) -> Vec<String> {
    let r = props(app, target);
    assert!(r.ok, "{target}: {:?}", r.error);
    r.value["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field"].as_str().unwrap().to_string())
        .collect()
}

/// A one-second mono WAV, written by hand so the test needs no audio crate.
fn write_wav(path: &std::path::Path, hz: f64) {
    let rate: u32 = 8000;
    let samples: Vec<i16> = (0..rate)
        .map(|i| ((i as f64 * hz * std::f64::consts::TAU / rate as f64).sin() * 8000.0) as i16)
        .collect();
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// A document with one of everything the panels edit: a plain shape, two graphic
/// symbols and an instance of the first, a text object, a person (armature),
/// one light of each kind, two sounds, and a later keyframe for tweening.
struct Fixture {
    app: App,
    shape: u64,
    instance: u64,
    text: u64,
    armature: u64,
    sun: u64,
    sky: u64,
    lamp: u64,
    gloom: u64,
    /// Target of a keyframe that carries a sound.
    sound_frame: String,
    /// The name of the other imported sound.
    other_sound: String,
    /// `layer:<index>` of the drawing layer.
    layer_target: String,
}

fn kind_id(objects: &[Value], kind: &str) -> u64 {
    objects
        .iter()
        .find(|o| o["kind"] == json!(kind))
        .unwrap_or_else(|| panic!("no {kind} in {objects:?}"))["id"]
        .as_u64()
        .unwrap()
}

fn fixture() -> Fixture {
    let mut app = App::new(GpuPreference::default());
    let r = call(
        &mut app,
        "script",
        json!({"code": "var d = fl.getDocumentDOM(); \
            d.addNewRectangle({left:10,top:10,right:110,bottom:90},0); d.selectAll(); \
            d.convertToSymbol('graphic','Box','center'); d.selectNone(); \
            d.addNewRectangle({left:200,top:200,right:300,bottom:280},0); d.selectNone();"}),
    );
    assert!(r.ok, "{:?}", r.error);
    // Give the first symbol a few frames, so an instance has a first frame to choose.
    assert!(call(&mut app, "gesture", json!({"tool": "selection", "points": pts(&[(60.0, 50.0)])})).ok);
    let mut steps = vec!["edit_symbol"];
    for _ in 0..6 {
        steps.extend(["next_frame", "insert_frame"]);
    }
    // A second, empty graphic to swap to.
    steps.extend(["edit_document", "first_frame", "deselect", "new_symbol", "edit_document", "first_frame", "deselect"]);
    for c in steps {
        assert!(cmd(&mut app, c).ok, "{c}");
    }
    assert!(call(&mut app, "gesture", json!({"tool": "text", "points": pts(&[(150.0, 150.0)])})).ok);
    let found = objects(&mut app);
    let shapes: Vec<u64> = found
        .iter()
        .filter(|o| o["kind"] == json!("shape"))
        .map(|o| o["id"].as_u64().unwrap())
        .collect();
    assert_eq!(shapes.len(), 2, "{found:?}");
    // The instance of "Box", the symbol that was given frames.
    let instance = found
        .iter()
        .filter(|o| o["kind"] == json!("instance"))
        .map(|o| o["id"].as_u64().unwrap())
        .find(|id| read(&mut app, &format!("element:{id}"), "symbol") == Some(json!("Box")))
        .unwrap_or_else(|| panic!("an instance of Box in {found:?}"));
    // Which of the two shapes carries text is read from its properties.
    let (shape, text) = if field_names(&mut app, &format!("element:{}", shapes[0]))
        .contains(&"text".to_string())
    {
        (shapes[1], shapes[0])
    } else {
        (shapes[0], shapes[1])
    };

    for c in ["add_sun", "add_sky", "add_lamp", "add_gloom"] {
        assert!(cmd(&mut app, c).ok, "{c}");
    }
    let (mut sun, mut sky, mut lamp, mut gloom) = (0, 0, 0, 0);
    for i in 0..64u64 {
        if !props(&mut app, &format!("light:{i}")).ok {
            continue;
        }
        let names = field_names(&mut app, &format!("light:{i}"));
        let has = |f: &str| names.iter().any(|n| n == f);
        if has("azimuth") {
            sun = i;
        } else if has("horizon") {
            sky = i;
        } else if has("radius") {
            lamp = i;
        } else if has("throw") {
            gloom = i;
        }
    }
    assert!(sun > 0 && sky > 0 && lamp > 0 && gloom > 0, "{sun} {sky} {lamp} {gloom}");

    assert!(cmd(&mut app, "add_person").ok);
    let armature = kind_id(&objects(&mut app), "armature");

    // A later keyframe on the person's layer, so a tween has somewhere to run to.
    for _ in 0..5 {
        assert!(cmd(&mut app, "next_frame").ok);
    }
    assert!(cmd(&mut app, "insert_keyframe").ok);
    assert!(cmd(&mut app, "first_frame").ok);

    let dir = std::env::temp_dir().join(format!("buzz_props_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut sound_names = Vec::new();
    for (file, hz) in [("alpha_tone.wav", 220.0), ("beta_tone.wav", 330.0)] {
        let path = dir.join(file);
        write_wav(&path, hz);
        let r = call(
            &mut app,
            "command",
            json!({"name": "import_sound", "path": path.display().to_string()}),
        );
        assert!(r.ok, "{:?}", r.error);
        sound_names.push(file.trim_end_matches(".wav").to_string());
    }
    let layers = call(&mut app, "inspect", json!({"what": "timeline"})).value["layers"].clone();
    let mut found_sound = None;
    for layer in layers.as_array().unwrap() {
        let target = format!("frame:{}:0", layer["name"].as_str().unwrap());
        if field_names(&mut app, &target).contains(&"sound_sync".to_string()) {
            let name = read(&mut app, &target, "sound").unwrap();
            found_sound = Some((target, name));
            break;
        }
    }
    let (sound_frame, sound_name) = found_sound.expect("an imported sound sits on a keyframe");
    let other_sound = sound_names
        .into_iter()
        .find(|n| sound_name.as_str().is_some_and(|s| !s.contains(n.as_str())))
        .expect("two different sounds");

    let index = layers
        .as_array()
        .unwrap()
        .iter()
        .position(|l| l["name"] == json!("Layer_1"))
        .expect("the drawing layer");
    let layer_target = format!("layer:{index}");

    Fixture { app, shape, instance, text, armature, sun, sky, lamp, gloom, sound_frame, other_sound, layer_target }
}

/// The target string to exercise one table entry on.
fn target_for(fx: &Fixture, kind: &str, field: &str) -> String {
    match kind {
        "document" | "camera" | "scene" => kind.to_string(),
        "light_rig" => "light:rig".to_string(),
        // By index: the layer's own name is one of the fields under test.
        "layer" => fx.layer_target.clone(),
        "frame" if field.starts_with("sound") => fx.sound_frame.clone(),
        "frame" => "frame:Person 1:0".to_string(),
        "light" => {
            let id = match field {
                "azimuth" | "elevation" => fx.sun,
                "horizon" => fx.sky,
                "x" | "y" | "height" | "radius" => fx.lamp,
                "edge_x" | "edge_y" | "facing" | "throw" | "width" => fx.gloom,
                _ => fx.lamp,
            };
            format!("light:{id}")
        }
        "element" => {
            let id = if field.starts_with("bone.") {
                fx.armature
            } else if field.starts_with("text") {
                fx.text
            } else if matches!(field, "blend" | "rotate_x" | "rotate_y" | "rotate_z" | "z") {
                fx.shape
            } else {
                fx.instance
            };
            format!("element:{id}")
        }
        other => panic!("unknown kind {other}"),
    }
}

/// Values to try for a field, in order; the first that is accepted is used.
fn candidates(field: &str, current: &Value, fx: &Fixture) -> Vec<Value> {
    if let Some(b) = current.as_bool() {
        return vec![json!(!b)];
    }
    let words = |list: &[&str]| list.iter().map(|s| json!(s)).collect::<Vec<_>>();
    match field {
        "kind" => return words(&["mask", "guide"]),
        "blend" => return words(&["multiply", "screen", "add", "overlay"]),
        "tween" => return words(&["classic", "motion"]),
        "ease" => return vec![json!(12.0), json!(40.0), json!([0.2, 0.1, 0.8, 0.9])],
        "loop_mode" => return words(&["play_once", "single_frame"]),
        "color_effect" => return words(&["tint", "alpha", "brightness"]),
        "sound_sync" => return words(&["event", "start", "stop", "stream"]),
        "fall" => return words(&["wall", "ground"]),
        "edges" => return words(&["off", "shapes", "figure"]),
        "model" => return words(&["form", "crescent"]),
        "text_align" => return words(&["centre", "right", "left"]),
        "follows" => return words(&["Person 1"]),
        "symbol" => return words(&["Symbol", "Box"]),
        "label" => return words(&["intro", "outro"]),
        "text" => return words(&["Hello", "World"]),
        "text_font" => return words(&["Arial", "Segoe UI", "Consolas"]),
        "highlight" => return words(&["#336699"]),
        "sound" => return vec![json!(fx.other_sound)],
        "placement" => return vec![json!(0), json!(1), json!(2)],
        "name" | "bone.0.name" => return words(&["Renamed", "Renamed Again"]),
        _ => {}
    }
    if current.as_str().is_some_and(|s| s.starts_with('#')) {
        return words(&["#336699", "#993366"]);
    }
    [0.05, 0.25, 0.5, 5.0, 12.0, 30.0, 1.5, 300.0, 1000.0].iter().map(|n| json!(n)).collect()
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 1e-3,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::String(x), Value::String(y)) => x.eq_ignore_ascii_case(y),
        _ => a == b,
    }
}

/// Entries that are not one undo step, with why.
const NOT_UNDOABLE: &[(&str, &str)] =
    &[("scene.name", "renaming a scene is not in the undo history, as in the scene tabs")];

#[test]
fn every_property_field_is_settable_readable_and_one_undo_step() {
    let mut fx = fixture();
    let mut by_kind: std::collections::BTreeMap<&str, usize> = Default::default();
    for &(kind, field, hint, panel) in PROPERTY_FIELDS {
        assert!(!hint.is_empty() && !panel.is_empty(), "{kind}.{field} needs a hint and a panel");
        let field = field.replace("<n>", "0");
        let field = field.as_str();
        // Moving the sound moves the keyframe the target names, so the target
        // cannot be read back afterwards; it has a test of its own.
        if (kind, field) == ("frame", "sound_start") {
            continue;
        }
        // The symbol-set fields read back lists and act on names, so a value
        // written is not the value read; they have a test of their own.
        if kind == "element"
            && matches!(field, "symbol_set" | "pick_from_set" | "include_in_set" | "remove_from_set")
        {
            continue;
        }
        let target = target_for(&fx, kind, field);

        // Some fields apply only while another is set a certain way.
        let mut prerequisites = 0;
        let prerequisite = match (kind, field) {
            ("element", "tint" | "tint_amount") => Some(("color_effect", json!("tint"))),
            ("element", "bone.0.min_angle" | "bone.0.max_angle") => {
                Some(("bone.0.limited", json!(true)))
            }
            _ => None,
        };
        if let Some((pre_field, value)) = prerequisite {
            let r = set(&mut fx.app, &target, pre_field, value);
            assert!(r.ok, "{kind}.{field} prerequisite {pre_field}: {:?}", r.error);
            prerequisites = 1;
        }

        let before = read(&mut fx.app, &target, field)
            .unwrap_or_else(|| panic!("{kind}.{field}: not listed by inspect properties for {target}"));
        let depth = undo_depth(&mut fx.app);

        let mut done = None;
        for candidate in candidates(field, &before, &fx).into_iter().filter(|c| !same(c, &before)) {
            let r = set(&mut fx.app, &target, field, candidate.clone());
            if r.ok {
                done = Some((candidate, r));
                break;
            }
            // A refused value changes nothing.
            assert_eq!(
                undo_depth(&mut fx.app),
                depth,
                "{kind}.{field} refused {candidate} but changed history"
            );
            assert_eq!(read(&mut fx.app, &target, field).as_ref(), Some(&before), "{kind}.{field}");
        }
        let (candidate, result) =
            done.unwrap_or_else(|| panic!("{kind}.{field} accepted none of its candidates"));
        let app = &mut fx.app;
        let now = read(app, &target, field).unwrap();
        assert!(same(&now, &candidate), "{kind}.{field}: set {candidate}, read back {now}");

        let undoable = !NOT_UNDOABLE.iter().any(|(k, _)| *k == format!("{kind}.{field}"));
        if undoable {
            assert_eq!(result.value["undo_steps_added"], json!(1), "{kind}.{field}: {:?}", result.value);
            assert_eq!(undo_depth(app), depth + 1, "{kind}.{field} is one undo step");
            assert!(cmd(app, "undo").ok);
            let restored = read(app, &target, field).unwrap();
            assert!(same(&restored, &before), "{kind}.{field}: undo restored {restored}, was {before}");
        } else {
            assert_eq!(undo_depth(app), depth, "{kind}.{field} is outside the history");
            let r = set(app, &target, field, before.clone());
            assert!(r.ok, "{kind}.{field} restore: {:?}", r.error);
        }
        for _ in 0..prerequisites {
            assert!(cmd(app, "undo").ok);
        }
        *by_kind.entry(kind).or_default() += 1;
    }
    eprintln!("fields covered: {} {by_kind:?}", PROPERTY_FIELDS.len());
}

#[test]
fn bad_property_calls_are_results_and_change_nothing() {
    let mut fx = fixture();
    let (shape, instance, armature, sun, gloom) = (fx.shape, fx.instance, fx.armature, fx.sun, fx.gloom);
    let app = &mut fx.app;
    let depth = undo_depth(app);
    let doc_before = props(app, "document").value;
    let cases: Vec<(String, &str, Value, &str)> = vec![
        ("nowhere".into(), "width", json!(10), "valid:"),
        ("layer:No Such Layer".into(), "visible", json!(true), "layers:"),
        ("element:999999".into(), "blend", json!("normal"), "no element"),
        ("frame:Person 1:99999".into(), "label", json!("x"), "past the end"),
        ("light:99999".into(), "intensity", json!(1.0), "no light"),
        ("document".into(), "no_such_field", json!(1), "valid:"),
        ("document".into(), "width", json!("wide"), "number"),
        ("document".into(), "width", json!(0), "between"),
        ("document".into(), "width", json!(1.0e9), "between"),
        ("document".into(), "background", json!("red"), "colour"),
        ("document".into(), "sort_by_depth", json!("maybe"), "true or false"),
        ("document".into(), "post.grade.gamma", json!(99), "between"),
        ("layer:Layer_1".into(), "kind", json!("banana"), "one of"),
        ("layer:Layer_1".into(), "follows", json!("No Such Layer"), "no layer"),
        ("layer:Layer_1".into(), "alpha", json!(150), "between"),
        ("camera".into(), "pitch", json!(89), "between"),
        ("camera".into(), "focal_distance", json!(10), "between"),
        ("frame:Person 1:0".into(), "sound_volume", json!(0.5), "no sound"),
        ("frame:Person 1:0".into(), "tween", json!("sideways"), "one of"),
        ("frame:Person 1:0".into(), "ease", json!([2, 0, 0, 1]), "ease"),
        (format!("element:{shape}"), "symbol", json!("Box"), "valid:"),
        (format!("element:{shape}"), "rotate_x", json!(90), "between"),
        (format!("element:{instance}"), "tint", json!("#ff0000"), "color_effect"),
        (format!("element:{instance}"), "symbol", json!("No Such Symbol"), "no symbol"),
        (format!("element:{armature}"), "bone.0.min_angle", json!(-10), "limited"),
        (format!("element:{armature}"), "bone.999.pinned", json!(true), "valid:"),
        (format!("element:{armature}"), "bone.0.angle", json!(400), "between"),
        (format!("element:{armature}"), "bone.0.angle", json!("up"), "number"),
        (format!("light:{sun}"), "radius", json!(100), "valid:"),
        (format!("light:{sun}"), "intensity", json!(9), "between"),
        (format!("light:{gloom}"), "intensity", json!(2), "between"),
        ("light:rig".into(), "edges", json!("sideways"), "one of"),
        ("scene".into(), "name", json!(""), "non-empty"),
    ];
    for (target, field, value, expect) in &cases {
        let r = set(app, target, field, value.clone());
        assert!(!r.ok, "{target}.{field}={value} should be refused");
        let message = r.error.unwrap();
        assert!(message.contains(expect), "{target}.{field}: '{message}' lacks '{expect}'");
        assert_eq!(undo_depth(app), depth, "{target}.{field} changed history");
    }
    assert_eq!(props(app, "document").value, doc_before, "nothing changed");

    assert!(!call(app, "set_property", json!({})).ok);
    assert!(!call(app, "set_property", json!({"target": "document"})).ok);
    assert!(!call(app, "set_property", json!({"target": "document", "field": "width"})).ok);
    assert!(!props(app, "nowhere").ok);
    assert!(!call(app, "inspect", json!({"what": "properties"})).ok);
}

/// **R1 of `docs/SERIES_EP1_PLAN.md`**: `bone.<n>.angle` poses a bone at the
/// playhead, in degrees against its parent, as one undo step, on the keyframe
/// that owns the playhead (so a later keyframe's pose is its own).
#[test]
fn bone_angle_poses_a_bone_at_the_playhead_in_one_undo_step() {
    let mut fx = fixture();
    let target = format!("element:{}", fx.armature);
    let app = &mut fx.app;
    let start = read(app, &target, "bone.1.angle").expect("bone.1.angle is listed");
    let start = start.as_f64().unwrap();
    let depth = undo_depth(app);

    // Frame 0: set, read back in degrees, one undo step labelled like a drag.
    let r = set(app, &target, "bone.1.angle", json!(40));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["undo_steps_added"], json!(1), "{:?}", r.value);
    assert_eq!(r.value["undo_label"], json!("Pose"));
    assert_eq!(undo_depth(app), depth + 1);
    assert!(same(&read(app, &target, "bone.1.angle").unwrap(), &json!(40)));

    // The rig's own pose agrees: the drawn bone really turned.
    let list = edit(app, &target, "bones", "get", json!({}));
    assert!(list.ok, "{:?}", list.error);

    // The later keyframe (frame 5) has its own pose.
    assert!(set(app, "document", "playhead", json!(5)).ok);
    let at5 = read(app, &target, "bone.1.angle").unwrap().as_f64().unwrap();
    assert!((at5 - start).abs() < 1e-3, "frame 5 kept the pose it was keyed with ({start}), was {at5}");
    assert!(set(app, &target, "bone.1.angle", json!(-25)).ok);
    assert!(same(&read(app, &target, "bone.1.angle").unwrap(), &json!(-25)));
    assert!(set(app, "document", "playhead", json!(0)).ok);
    assert!(same(&read(app, &target, "bone.1.angle").unwrap(), &json!(40)), "frame 0 keeps its pose");

    // Limits clamp it, as a drag is clamped.
    assert!(set(app, &target, "bone.1.limited", json!(true)).ok);
    assert!(set(app, &target, "bone.1.angle", json!(150)).ok);
    assert!(same(&read(app, &target, "bone.1.angle").unwrap(), &json!(90)));

    // Undo walks back through every step.
    while undo_depth(app) > depth {
        assert!(cmd(app, "undo").ok);
    }
    assert!(same(&read(app, &target, "bone.1.angle").unwrap(), &json!(start)));
}

/// `docs/SYMBOL_SET_PLAN.md` S5: the agent builds a set, picks from it at the
/// playhead and takes a member out again, each one undo step, exactly as the
/// Properties panel's Set row and Include… menu do.
#[test]
fn a_symbol_set_is_built_picked_and_undone_through_set_property() {
    let mut fx = fixture();
    let target = format!("element:{}", fx.instance);
    let app = &mut fx.app;
    // Every layer long enough that frame 3 is inside the instance's span.
    assert!(set(app, "document", "frame_count", json!(12)).ok);
    let depth = undo_depth(app);

    assert_eq!(read(app, &target, "symbol_set"), Some(json!([])));
    let offered = read(app, &target, "include_in_set").unwrap();
    assert!(offered.as_array().unwrap().contains(&json!("Symbol")), "{offered}");
    assert!(!set(app, &target, "symbol_set", json!(["Box", "Symbol"])).ok, "read-only");
    assert!(!set(app, &target, "pick_from_set", json!("Symbol")).ok, "no set yet");
    assert_eq!(undo_depth(app), depth);

    // Include.
    let r = set(app, &target, "include_in_set", json!("Symbol"));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["undo_steps_added"], json!(1), "{:?}", r.value);
    assert_eq!(read(app, &target, "symbol_set"), Some(json!(["Box", "Symbol"])));
    assert!(!set(app, &target, "include_in_set", json!("Symbol")).ok, "already in");

    // Refused picks change nothing.
    let depth_set = undo_depth(app);
    for bad in [json!("Box"), json!("No Such Symbol"), json!({"symbol": "Symbol", "start": "sideways"})] {
        assert!(!set(app, &target, "pick_from_set", bad.clone()).ok, "{bad}");
        assert_eq!(undo_depth(app), depth_set, "{bad} changed history");
    }

    // Pick at frame 3: frame 3 shows the pick, frame 0 keeps Box.
    assert!(set(app, "document", "playhead", json!(3)).ok);
    let r = set(app, &target, "pick_from_set", json!({"symbol": "Symbol", "start": "from_start"}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["undo_steps_added"], json!(1), "{:?}", r.value);
    assert_eq!(read(app, &target, "pick_from_set"), Some(json!("Symbol")));
    assert_eq!(read(app, &target, "first_frame"), Some(json!(1)), "from its first frame");
    assert!(set(app, "document", "playhead", json!(0)).ok);
    assert_eq!(read(app, &target, "pick_from_set"), Some(json!("Box")), "earlier frames keep Box");

    // Another instance edit on the picked frame keeps the pick: every field
    // is read from, and written back to, the copy at the playhead.
    assert!(set(app, "document", "playhead", json!(3)).ok);
    assert!(set(app, &target, "sync_to_root", json!(true)).ok);
    assert_eq!(read(app, &target, "pick_from_set"), Some(json!("Symbol")), "the edit kept the pick");
    assert_eq!(read(app, &target, "symbol"), Some(json!("Symbol")));
    assert!(cmd(app, "undo").ok);

    // Undo the pick: frame 3 shows Box again.
    assert!(cmd(app, "undo").ok);
    assert!(set(app, "document", "playhead", json!(3)).ok);
    assert_eq!(read(app, &target, "pick_from_set"), Some(json!("Box")));

    // Remove: never the owner, never the symbol shown.
    assert_eq!(read(app, &target, "remove_from_set"), Some(json!(["Symbol"])));
    assert!(!set(app, &target, "remove_from_set", json!("Box")).ok, "the owner stays");
    let r = set(app, &target, "remove_from_set", json!("Symbol"));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["undo_steps_added"], json!(1), "{:?}", r.value);
    assert_eq!(read(app, &target, "symbol_set"), Some(json!([])));

    while undo_depth(app) > depth {
        assert!(cmd(app, "undo").ok);
    }
    assert_eq!(read(app, &target, "symbol_set"), Some(json!([])));
}

#[test]
fn inspect_properties_lists_every_field_with_a_hint_and_the_table_is_honest() {
    let mut fx = fixture();
    let targets = [
        "document".to_string(),
        "scene".to_string(),
        "camera".to_string(),
        "light:rig".to_string(),
        "layer:Layer_1".to_string(),
        "frame:Person 1:0".to_string(),
        fx.sound_frame.clone(),
        format!("element:{}", fx.shape),
        format!("element:{}", fx.instance),
        format!("element:{}", fx.text),
        format!("element:{}", fx.armature),
        format!("light:{}", fx.sun),
        format!("light:{}", fx.sky),
        format!("light:{}", fx.lamp),
        format!("light:{}", fx.gloom),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for target in &targets {
        let r = props(&mut fx.app, target);
        assert!(r.ok, "{target}: {:?}", r.error);
        let kind = r.value["kind"].as_str().unwrap().to_string();
        for f in r.value["fields"].as_array().unwrap() {
            let name = f["field"].as_str().unwrap();
            assert!(!f["hint"].as_str().unwrap().is_empty(), "{target}.{name} has no hint");
            let key = match name.strip_prefix("bone.") {
                Some(rest) => format!("bone.<n>.{}", rest.split_once('.').unwrap().1),
                None => name.to_string(),
            };
            assert!(
                PROPERTY_FIELDS.iter().any(|(k, field, ..)| *k == kind && *field == key),
                "{target}.{name} is listed by inspect but missing from PROPERTY_FIELDS"
            );
            seen.insert((kind.clone(), key));
        }
    }
    // Conditional fields (colour effect, bone limits) are listed once their condition
    // holds, which the every-field test exercises; the rest are all listed outright.
    for &(kind, field, ..) in PROPERTY_FIELDS {
        let conditional = matches!(
            (kind, field),
            ("element", "brightness" | "alpha" | "tint" | "tint_amount")
                | ("element", "bone.<n>.min_angle" | "bone.<n>.max_angle")
        );
        assert!(
            conditional || seen.contains(&(kind.to_string(), field.to_string())),
            "{kind}.{field} never listed"
        );
    }

    assert!(PROPERTY_EXCLUDED.len() <= 14, "keep the exclusion list short");
    for (what, why) in PROPERTY_EXCLUDED {
        assert!(!what.is_empty() && why.len() > 20, "{what} needs an honest reason");
    }
    let cat = call(&mut fx.app, "inspect", json!({"what": "catalog"}));
    assert!(cat.value.as_array().unwrap().iter().any(|e| e["id"] == json!("set_property")));
}

// -- D2: dialog commands run from args; list-structured edits ---------------------------

use buzz_app::agent::AgentResult;
use buzz_app::agent::dialogs::STILL_NEEDS_DIALOG;

fn cmda(app: &mut App, name: &str, args: Value) -> AgentResult {
    call(app, "command", json!({"name": name, "args": args}))
}

fn edit(app: &mut App, target: &str, list: &str, op: &str, extra: Value) -> AgentResult {
    let mut args = json!({"target": target, "list": list, "op": op});
    if let Some(map) = extra.as_object() {
        for (k, v) in map {
            args[k] = v.clone();
        }
    }
    call(app, "edit_list", args)
}

fn items_of(r: &AgentResult) -> Vec<Value> {
    assert!(r.ok, "{:?}", r.error);
    r.value["items"].as_array().cloned().unwrap_or_default()
}

fn steps(r: &AgentResult) -> i64 {
    r.value["undo_steps_added"].as_i64().unwrap()
}

fn timeline_layers(app: &mut App) -> Vec<Value> {
    call(app, "inspect", json!({"what": "timeline"})).value["layers"].as_array().unwrap().clone()
}

fn keyframe_total(app: &mut App) -> usize {
    timeline_layers(app).iter().map(|l| l["keyframes"].as_array().unwrap().len()).sum()
}

fn expect_err(r: &AgentResult, needle: &str) {
    assert!(!r.ok, "expected a refusal containing '{needle}', got {:?}", r.value);
    let e = r.error.clone().unwrap();
    assert!(e.contains(needle), "'{e}' lacks '{needle}'");
}

fn undo_n(app: &mut App, n: usize) {
    for _ in 0..n {
        assert!(cmd(app, "undo").ok);
    }
}

/// A PNG of a black disc on white paper.
fn write_disc_png(path: &std::path::Path) {
    let size = 64u32;
    let c = size as f64 / 2.0;
    let mut pixels = Vec::new();
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2)).sqrt();
            let v = if d < 20.0 { 0u8 } else { 255 };
            pixels.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
}

#[test]
fn dialog_commands_say_which_args_are_missing_and_change_nothing() {
    let mut app = App::new(GpuPreference::default());
    let depth = undo_depth(&mut app);
    for (id, needle) in [
        ("perform", "action"),
        ("set_scene", "setting"),
        ("direct_scene", "story"),
        ("scenery_for", "scenery"),
        ("lip_sync", "mouth"),
        ("ai_direct_story", "brief"),
        ("ai_rig_vector_character", "symbol"),
        ("ai_identify_assets", "renames"),
        ("ai_character_from_image", "path"),
    ] {
        let r = cmd(&mut app, id);
        expect_err(&r, "missing required args");
        expect_err(&r, needle);
    }
    // Needing a selection is refused in the dialog's own words.
    expect_err(&cmd(&mut app, "trace_bitmap_options"), "Select a picture");
    expect_err(&cmd(&mut app, "add_follow_through"), "rigged character");
    expect_err(&cmd(&mut app, "add_wiggle"), "select an object");

    // A typo is refused, never ignored.
    for &c in Command::unit_commands() {
        if route(c) == Route::Args {
            let r = cmda(&mut app, c.id(), json!({"bogus": 1}));
            expect_err(&r, "unknown arg 'bogus'");
        }
    }
    assert_eq!(undo_depth(&mut app), depth, "nothing changed");

    // What still needs a dialog says why, and the catalog lists every arg schema.
    // (Nothing does since H2: Character from Image runs from its args.)
    assert!(STILL_NEEDS_DIALOG.iter().all(|(_, why)| why.len() > 40));
    let cat = call(&mut app, "inspect", json!({"what": "catalog"})).value;
    for e in cat.as_array().unwrap() {
        if e["how"] == json!("args") {
            assert!(e["args"].as_str().unwrap().starts_with('{'), "{e}");
        }
    }
    assert!(cat.as_array().unwrap().iter().any(|e| e["id"] == json!("edit_list")));
    // Every NeedsDialog route has a reason in the list, and nothing else does.
    for &c in Command::unit_commands() {
        let listed = STILL_NEEDS_DIALOG.iter().any(|(id, _)| *id == c.id());
        assert_eq!(route(c) == Route::NeedsDialog, listed, "{}", c.id());
    }
}

#[test]
fn new_runs_from_args_and_guards_unsaved_work() {
    let mut app = App::new(GpuPreference::default());
    expect_err(&cmda(&mut app, "new", json!({"width": 0})), "between");
    expect_err(&cmda(&mut app, "new", json!({"background": "red"})), "colour");

    let r = cmda(&mut app, "new", json!({"width": 1280, "height": 720, "fps": 30, "background": "#102030", "discard_changes": true}));
    assert!(r.ok, "{:?}", r.error);
    let doc = call(&mut app, "inspect", json!({"what": "document"})).value;
    assert_eq!((doc["width"].as_f64(), doc["height"].as_f64(), doc["fps"].as_f64()), (Some(1280.0), Some(720.0), Some(30.0)));
    assert_eq!(doc["dirty"], json!(false));
    assert_eq!(read(&mut app, "document", "background"), Some(json!("#102030")));

    // Unsaved work is not replaced without saying so.
    assert!(cmd(&mut app, "new_layer").ok);
    let layers = layer_count(&mut app);
    expect_err(&cmda(&mut app, "new", json!({"width": 640})), "unsaved");
    assert_eq!(layer_count(&mut app), layers);
    assert!(cmda(&mut app, "new", json!({"width": 640, "height": 360, "discard_changes": true})).ok);
    assert_eq!(call(&mut app, "inspect", json!({"what": "document"})).value["width"].as_f64(), Some(640.0));
}

#[test]
fn the_story_panel_commands_run_from_args() {
    let mut app = App::new(GpuPreference::default());
    let layers = layer_count(&mut app);

    expect_err(&cmda(&mut app, "set_scene", json!({"setting": "volcano"})), "unknown setting");
    expect_err(&cmda(&mut app, "set_scene", json!({"setting": "night", "horizon": 5})), "between");
    assert_eq!(layer_count(&mut app), layers, "refused calls change nothing");

    let r = cmda(&mut app, "set_scene", json!({"setting": "night", "cast": 2, "frames": 48}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    let after = timeline_layers(&mut app);
    assert!(after.len() >= layers + 4, "a backdrop, a ground and a cast of two: {} -> {}", layers, after.len());
    undo_n(&mut app, 1);
    assert_eq!(layer_count(&mut app), layers, "one undo takes the whole scene back");

    // Scenery goes into a set that is there.
    assert!(cmda(&mut app, "set_scene", json!({"setting": "daylight", "cast": 0})).ok);
    let before = layer_count(&mut app);
    expect_err(&cmda(&mut app, "scenery_for", json!({"scenery": "bare"})), "Bare");
    expect_err(&cmda(&mut app, "scenery_for", json!({"scenery": "forest", "art": {"roots": "x"}})), "unknown scenery part");
    expect_err(&cmda(&mut app, "scenery_for", json!({"scenery": "forest", "art": {"trees": "No Such"}})), "no trees symbol");
    let r = cmda(&mut app, "scenery_for", json!({"scenery": "forest"}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1);
    assert!(r.value["pieces"].as_u64().unwrap() > 0);
    assert!(layer_count(&mut app) > before, "trees stand on layers of their own");
    undo_n(&mut app, 1);
    assert_eq!(layer_count(&mut app), before);

    // Directing rebuilds the open shot from prose.
    expect_err(&cmda(&mut app, "direct_scene", json!({"story": "The rain fell and fell."})), "");
    let r = cmda(
        &mut app,
        "direct_scene",
        json!({"story": "Night. Ana walks in from the left. Ana talks to Ben for 3 seconds. Ben walks off right."}),
    );
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert!(layer_count(&mut app) >= 4, "a sky, a ground, Ana and Ben");
    assert!(keyframe_total(&mut app) > 10, "the performances are keyframed");
    undo_n(&mut app, 1);
    assert_eq!(layer_count(&mut app), before, "undo brings the shot back as it was");

    // As a film: a scene per paragraph.
    let scenes = call(&mut app, "inspect", json!({"what": "document"})).value["scenes"].as_array().unwrap().len();
    let r = cmda(
        &mut app,
        "direct_scene",
        json!({"sequence": true, "story": "Night. Ana walks in from the left.\n\nDaylight. Ben walks off right."}),
    );
    assert!(r.ok, "{:?}", r.error);
    assert!(r.value["shots"].as_u64().unwrap() >= 2, "{:?}", r.value);
    let after = call(&mut app, "inspect", json!({"what": "document"})).value["scenes"].as_array().unwrap().len();
    assert!(after > scenes, "{scenes} -> {after}");
}

#[test]
fn perform_follow_through_and_wiggle_run_from_args() {
    let mut app = App::new(GpuPreference::default());
    assert!(cmd(&mut app, "add_person").ok);
    let rig = kind_id(&objects(&mut app), "armature");
    let target = format!("element:{rig}");

    expect_err(&cmda(&mut app, "perform", json!({"action": "moonwalk"})), "unknown action");
    expect_err(&cmda(&mut app, "perform", json!({"action": "walk", "step": 0})), "between");
    expect_err(&cmda(&mut app, "perform", json!({"action": "walk", "element": 999999})), "no element");

    let keys = keyframe_total(&mut app);
    let r = cmda(&mut app, "perform", json!({"action": "walk", "from_frame": 0, "to_frame": 23, "distance": 200}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert!(keyframe_total(&mut app) > keys, "the walk is keyed");
    undo_n(&mut app, 1);
    assert_eq!(keyframe_total(&mut app), keys);

    // `element` selects as a click would.
    assert!(cmd(&mut app, "deselect").ok);
    let r = cmda(&mut app, "perform", json!({"action": "idle", "element": rig, "to_frame": 11}));
    assert!(r.ok, "{:?}", r.error);
    undo_n(&mut app, 1);

    // Live follow-through and wiggle are modifiers; baking writes keys.
    let bones = items_of(&edit(&mut app, &target, "bones", "get", json!({})));
    let root_name = bones[0]["name"].as_str().unwrap().to_string();
    expect_err(&cmda(&mut app, "add_follow_through", json!({"element": rig, "root": "No Such Bone"})), "no bone");
    let r = cmda(&mut app, "add_follow_through", json!({"element": rig, "root": root_name, "stiffness": 90}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1);
    let mods = items_of(&edit(&mut app, &target, "modifiers", "get", json!({})));
    assert!(mods.iter().any(|m| m["kind"] == json!("spring") && m["stiffness"] == json!(90.0)), "{mods:?}");
    undo_n(&mut app, 1);

    let r = cmda(&mut app, "add_wiggle", json!({"element": rig, "amplitude": 7, "frequency": 2}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1);
    let mods = items_of(&edit(&mut app, &target, "modifiers", "get", json!({})));
    assert!(mods.iter().any(|m| m["kind"] == json!("wiggle") && m["amplitude"] == json!(7.0)), "{mods:?}");
    undo_n(&mut app, 1);
    assert!(items_of(&edit(&mut app, &target, "modifiers", "get", json!({}))).is_empty());

    let keys = keyframe_total(&mut app);
    let r = cmda(&mut app, "add_wiggle", json!({"element": rig, "live": false, "to_frame": 11}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1);
    assert!(keyframe_total(&mut app) > keys, "a baked wiggle is keyframes, not a modifier");
    undo_n(&mut app, 1);
}

#[test]
fn lip_sync_runs_from_args() {
    let mut app = App::new(GpuPreference::default());
    let first = timeline_layers(&mut app)[0]["name"].as_str().unwrap().to_string();

    // No sound yet.
    assert!(cmd(&mut app, "new_mouth_symbol").ok);
    let lib = call(&mut app, "inspect", json!({"what": "library"})).value["symbols"].as_array().unwrap().clone();
    let mouth = lib.last().unwrap()["name"].as_str().unwrap().to_string();
    let depth = undo_depth(&mut app);
    expect_err(&cmda(&mut app, "lip_sync", json!({"mouth": mouth, "layer": first})), "no sound");
    assert_eq!(undo_depth(&mut app), depth);

    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("voice.wav");
    write_wav(&wav, 300.0);
    assert!(call(&mut app, "command", json!({"name": "import_sound", "path": wav.display().to_string()})).ok);

    expect_err(&cmda(&mut app, "lip_sync", json!({"mouth": "No Such Mouth", "layer": first})), "no mouth symbol");
    expect_err(&cmda(&mut app, "lip_sync", json!({"mouth": mouth, "layer": "No Layer"})), "no layer");
    expect_err(&cmda(&mut app, "lip_sync", json!({"mouth": mouth, "silence": 4})), "between");

    // What is on the mouth layer: every object across its keyframes.
    let drawn = |app: &mut App| -> u64 {
        timeline_layers(app)
            .iter()
            .filter(|l| l["name"] == json!(first))
            .flat_map(|l| l["keyframes"].as_array().unwrap().clone())
            .map(|k| k["objects"].as_u64().unwrap())
            .sum()
    };
    let before = drawn(&mut app);
    let depth = undo_depth(&mut app);
    let r = cmda(&mut app, "lip_sync", json!({"mouth": mouth, "layer": first, "hold": 1}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert_eq!(undo_depth(&mut app), depth + 1);
    assert!(drawn(&mut app) > before, "mouths were placed on the layer");
    undo_n(&mut app, 1);
    assert_eq!(drawn(&mut app), before);
}

#[test]
fn trace_bitmap_options_runs_from_args() {
    let mut app = App::new(GpuPreference::default());
    let dir = tempfile::tempdir().unwrap();
    let png_path = dir.path().join("disc.png");
    write_disc_png(&png_path);
    let r = call(&mut app, "command", json!({"name": "import_image", "path": png_path.display().to_string()}));
    assert!(r.ok, "{:?}", r.error);
    let picture = objects(&mut app)[0]["id"].as_u64().unwrap();

    expect_err(&cmda(&mut app, "trace_bitmap_options", json!({"colours": 1})), "between");
    expect_err(&cmda(&mut app, "trace_bitmap_options", json!({"mode": "sketchy"})), "unknown mode");
    expect_err(&cmda(&mut app, "trace_bitmap_options", json!({"element": 999999})), "no element");

    let depth = undo_depth(&mut app);
    let r = cmda(&mut app, "trace_bitmap_options", json!({"element": picture, "colours": 2, "mode": "outline", "speckle": 4}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert_eq!(undo_depth(&mut app), depth + 1);
    let after = objects(&mut app);
    assert!(!after.iter().any(|o| o["id"] == json!(picture)), "the picture was replaced by traced artwork: {after:?}");
    assert!(after.iter().any(|o| o["kind"] == json!("shape")), "{after:?}");
    undo_n(&mut app, 1);
    assert!(objects(&mut app).iter().any(|o| o["id"] == json!(picture)), "undo brings the picture back");
}

#[test]
fn identify_assets_applies_renames_and_the_ai_jobs_refuse_bad_args() {
    let mut app = App::new(GpuPreference::default());
    let r = call(
        &mut app,
        "script",
        json!({"code": "var d = fl.getDocumentDOM(); d.addNewRectangle({left:10,top:10,right:60,bottom:60},0); \
                        d.selectAll(); d.convertToSymbol('graphic','Box','center'); d.selectNone();"}),
    );
    assert!(r.ok, "{:?}", r.error);
    let names = |app: &mut App| -> Vec<String> {
        call(app, "inspect", json!({"what": "library"})).value["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(names(&mut app).contains(&"Box".to_string()));

    expect_err(&cmda(&mut app, "ai_identify_assets", json!({"renames": []})), "non-empty");
    expect_err(&cmda(&mut app, "ai_identify_assets", json!({"renames": [{"symbol": "Nope", "name": "x"}]})), "no renamed symbol");
    expect_err(&cmda(&mut app, "ai_identify_assets", json!({"renames": [{"symbol": "Box"}]})), "'name'");
    expect_err(&cmda(&mut app, "ai_identify_assets", json!({"apply_proposals": true})), "no proposals");

    let depth = undo_depth(&mut app);
    let r = cmda(&mut app, "ai_identify_assets", json!({"renames": [{"symbol": "Box", "name": "crate", "tags": ["prop"]}]}));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert_eq!(undo_depth(&mut app), depth + 1);
    assert!(names(&mut app).contains(&"crate".to_string()) && !names(&mut app).contains(&"Box".to_string()));
    undo_n(&mut app, 1);
    assert!(names(&mut app).contains(&"Box".to_string()));

    // The network / GPU jobs validate before they start anything.
    expect_err(&cmda(&mut app, "ai_rig_vector_character", json!({"symbol": "No Such Symbol"})), "no character symbol");
    expect_err(&cmda(&mut app, "ai_direct_story", json!({"brief": "   "})), "missing required args");
    assert_eq!(undo_depth(&mut app), depth);
}

#[test]
fn edit_list_filters_on_an_element_and_a_layer() {
    let mut fx = fixture();
    let shape = format!("element:{}", fx.shape);
    let layer = fx.layer_target.clone();
    let app = &mut fx.app;
    for target in [shape.as_str(), layer.as_str()] {
        assert!(items_of(&edit(app, target, "filters", "get", json!({}))).is_empty(), "{target}");
        let depth = undo_depth(app);

        let r = edit(app, target, "filters", "add", json!({"item": {"kind": "blur"}}));
        assert_eq!((items_of(&r).len(), steps(&r)), (1, 1), "{target}: {:?}", r.value);
        assert_eq!(items_of(&r)[0]["kind"], json!("blur"));
        assert_eq!(items_of(&r)[0]["params"]["x"], json!(5.0), "the panel's own defaults");

        let r = edit(
            app,
            target,
            "filters",
            "add",
            json!({"item": {"kind": "Drop Shadow", "color": "#336699", "distance": 9, "quality": "high", "enabled": false}}),
        );
        assert_eq!((items_of(&r).len(), steps(&r)), (2, 1), "{:?}", r.value);
        let shadow = &items_of(&r)[1];
        assert_eq!(shadow["kind"], json!("drop_shadow"));
        assert_eq!(shadow["enabled"], json!(false));
        assert_eq!(shadow["params"]["color"], json!("#336699"));
        assert_eq!(shadow["params"]["distance"], json!(9.0));
        assert_eq!(shadow["params"]["quality"], json!("High"));

        let r = edit(app, target, "filters", "set", json!({"index": 0, "item": {"x": 2, "enabled": false}}));
        assert_eq!(steps(&r), 1, "{:?}", r.value);
        assert_eq!(items_of(&r)[0]["params"]["x"], json!(2.0));
        assert_eq!(items_of(&r)[0]["params"]["y"], json!(5.0), "other parameters keep their values");
        assert_eq!(items_of(&r)[0]["enabled"], json!(false));

        let r = edit(app, target, "filters", "move", json!({"index": 0, "to": 1}));
        assert_eq!(steps(&r), 1, "{:?}", r.value);
        assert_eq!(items_of(&r)[0]["kind"], json!("drop_shadow"));
        assert_eq!(items_of(&r)[1]["kind"], json!("blur"));

        let r = edit(app, target, "filters", "remove", json!({"index": 0}));
        assert_eq!((items_of(&r).len(), steps(&r)), (1, 1));
        assert_eq!(items_of(&r)[0]["kind"], json!("blur"));
        assert_eq!(undo_depth(app), depth + 5, "{target}: one undo step per op");

        // Each op undoes by itself, back to the empty list.
        undo_n(app, 1);
        assert_eq!(items_of(&edit(app, target, "filters", "get", json!({}))).len(), 2, "{target}: undid the remove");
        undo_n(app, 4);
        assert!(items_of(&edit(app, target, "filters", "get", json!({}))).is_empty(), "{target}: undo restored the list");
        assert_eq!(undo_depth(app), depth);

        // Bad ops change nothing.
        assert!(edit(app, target, "filters", "add", json!({"item": {"kind": "blur"}})).ok);
        let depth = undo_depth(app);
        let before = edit(app, target, "filters", "get", json!({})).value["items"].clone();
        expect_err(&edit(app, target, "filters", "add", json!({"item": {"kind": "sparkle"}})), "unknown filter kind");
        expect_err(&edit(app, target, "filters", "add", json!({"item": {}})), "needs a 'kind'");
        expect_err(&edit(app, target, "filters", "add", json!({})), "'item'");
        expect_err(&edit(app, target, "filters", "set", json!({"index": 0, "item": {"radius": 3}})), "unknown parameter 'radius'");
        expect_err(&edit(app, target, "filters", "set", json!({"index": 0, "item": {"x": "wide"}})), "bad value");
        expect_err(&edit(app, target, "filters", "set", json!({"index": 0, "item": {"kind": "glow"}})), "cannot become");
        expect_err(&edit(app, target, "filters", "set", json!({"index": 7, "item": {"x": 1}})), "out of range");
        expect_err(&edit(app, target, "filters", "set", json!({"item": {"x": 1}})), "needs 'index'");
        expect_err(&edit(app, target, "filters", "remove", json!({"index": 3})), "out of range");
        expect_err(&edit(app, target, "filters", "move", json!({"index": 0, "to": 4})), "needs 'to'");
        expect_err(&edit(app, target, "filters", "move", json!({"index": 0, "to": 0})), "already there");
        assert_eq!(undo_depth(app), depth);
        assert_eq!(edit(app, target, "filters", "get", json!({})).value["items"], before);
        undo_n(app, 1);
    }

    // The wrong place for a list, a list that does not exist, an op it lacks.
    expect_err(&edit(app, "camera", "filters", "get", json!({})), "lives on element");
    expect_err(&edit(app, "nowhere", "filters", "get", json!({})), "unknown target");
    expect_err(&edit(app, &shape, "sparkles", "get", json!({})), "unknown list");
    expect_err(&edit(app, &shape, "filters", "shuffle", json!({})), "no op 'shuffle'");
    expect_err(&edit(app, &shape, "modifiers", "move", json!({"index": 0})), "no op 'move'");
    assert!(!call(app, "edit_list", json!({"target": "document"})).ok);
    expect_err(&call(app, "edit_list", json!({"target": shape, "list": "filters", "op": "get", "bogus": 1})), "unknown arg");
}

#[test]
fn edit_list_live_modifiers() {
    let mut fx = fixture();
    let instance = format!("element:{}", fx.instance);
    let armature = format!("element:{}", fx.armature);
    let app = &mut fx.app;
    let depth = undo_depth(app);

    let r = edit(app, &instance, "modifiers", "add", json!({"item": {"kind": "breathe"}}));
    assert_eq!((items_of(&r).len(), steps(&r)), (1, 1), "{:?}", r.value);
    assert_eq!(items_of(&r)[0]["rate"], json!(14.0), "the panel's own defaults");
    let r = edit(app, &instance, "modifiers", "add", json!({"item": {"kind": "wiggle", "amplitude": 3}}));
    assert_eq!((items_of(&r).len(), steps(&r)), (2, 1));
    assert_eq!(items_of(&r)[1]["amplitude"], json!(3.0));
    assert_eq!(items_of(&r)[1]["frequency"], json!(1.5));
    let r = edit(app, &instance, "modifiers", "set", json!({"index": 1, "item": {"frequency": 4}}));
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert_eq!(items_of(&r)[1]["frequency"], json!(4.0));
    assert_eq!(items_of(&r)[1]["amplitude"], json!(3.0));
    // A different kind at an index starts from that kind's defaults.
    let r = edit(app, &instance, "modifiers", "set", json!({"index": 1, "item": {"kind": "sway"}}));
    assert_eq!(items_of(&r)[1]["kind"], json!("sway"));
    let r = edit(app, &instance, "modifiers", "remove", json!({"index": 0}));
    assert_eq!((items_of(&r).len(), steps(&r)), (1, 1));
    assert_eq!(items_of(&r)[0]["kind"], json!("sway"));
    assert_eq!(undo_depth(app), depth + 5, "one undo step per op");
    undo_n(app, 5);
    assert!(items_of(&edit(app, &instance, "modifiers", "get", json!({}))).is_empty());

    // A spring needs a rig and a real bone.
    expect_err(&edit(app, &instance, "modifiers", "add", json!({"item": {"kind": "spring"}})), "not an armature");
    expect_err(&edit(app, &armature, "modifiers", "add", json!({"item": {"kind": "spring", "root": 999}})), "not a bone");
    let r = edit(app, &armature, "modifiers", "add", json!({"item": {"kind": "spring", "root": 0, "stiffness": 60}}));
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert_eq!(items_of(&r)[0]["stiffness"], json!(60.0));
    undo_n(app, 1);

    let depth = undo_depth(app);
    expect_err(&edit(app, &instance, "modifiers", "add", json!({"item": {"kind": "sparkle"}})), "unknown modifier kind");
    expect_err(&edit(app, &instance, "modifiers", "add", json!({"item": {"kind": "wiggle", "speed": 1}})), "unknown parameter 'speed'");
    expect_err(&edit(app, &instance, "modifiers", "add", json!({"item": {"kind": "wiggle", "amplitude": "big"}})), "needs a number");
    expect_err(&edit(app, &instance, "modifiers", "remove", json!({"index": 0})), "empty");
    expect_err(&edit(app, "layer:0", "modifiers", "get", json!({})), "lives on element");
    assert_eq!(undo_depth(app), depth);
}

#[test]
fn edit_list_swatches_and_their_folders() {
    let mut fx = fixture();
    let app = &mut fx.app;
    let base = items_of(&edit(app, "document", "swatches", "get", json!({})));
    let base_folders = items_of(&edit(app, "document", "swatch_folders", "get", json!({})));
    let depth = undo_depth(app);

    let r = edit(app, "document", "swatches", "add", json!({"item": {"name": "Coat", "color": "#336699"}}));
    assert_eq!((items_of(&r).len(), steps(&r)), (base.len() + 1, 1), "{:?}", r.value);
    let coat = items_of(&r).into_iter().find(|s| s["name"] == json!("Coat")).unwrap();
    assert_eq!(coat["color"], json!("#336699"));
    let index = coat["index"].as_u64().unwrap();

    let r = edit(app, "document", "swatches", "set", json!({"index": index, "item": {"name": "Jacket"}}));
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert!(items_of(&r).iter().any(|s| s["name"] == json!("Jacket")));
    // Setting the colour repaints what is linked to it, and says how many.
    let r = edit(app, "document", "swatches", "set", json!({"index": index, "item": {"color": "#993366"}}));
    assert_eq!(steps(&r), 1);
    assert_eq!(r.value["repainted"], json!(0));
    assert!(items_of(&r).iter().any(|s| s["name"] == json!("Jacket") && s["color"] == json!("#993366")));

    // Folders: make one, move the swatch in, then delete the folder keeping the colour.
    let r = edit(app, "document", "swatch_folders", "add", json!({"item": {"name": "Hero/Skin"}}));
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    let paths: Vec<Value> = items_of(&r).iter().map(|f| f["path"].clone()).collect();
    assert!(paths.contains(&json!("Hero")) && paths.contains(&json!("Hero/Skin")), "{paths:?}");
    let r = edit(app, "document", "swatches", "move", json!({"index": index, "to": "Hero/Skin"}));
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert!(items_of(&r).iter().any(|s| s["name"] == json!("Jacket") && s["folder"] == json!("Hero/Skin")));
    let r = edit(app, "document", "swatch_folders", "remove", json!({"item": {"name": "Hero"}}));
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert!(!items_of(&r).iter().any(|f| f["path"].as_str().unwrap().starts_with("Hero")));
    let now = items_of(&edit(app, "document", "swatches", "get", json!({})));
    assert!(
        now.iter().any(|s| s["name"] == json!("Jacket") && s["folder"].is_null()),
        "the colours stay, at the root: {now:?}"
    );

    let r = edit(app, "document", "swatches", "remove", json!({"index": index}));
    assert_eq!((items_of(&r).len(), steps(&r)), (base.len(), 1));
    assert_eq!(undo_depth(app), depth + 7, "one undo step per op");

    // Undoing every op restores the palette exactly (swatches are in the history).
    undo_n(app, 7);
    assert_eq!(items_of(&edit(app, "document", "swatches", "get", json!({}))), base);
    assert_eq!(items_of(&edit(app, "document", "swatch_folders", "get", json!({}))), base_folders);

    let depth = undo_depth(app);
    expect_err(&edit(app, "document", "swatches", "add", json!({"item": {"name": "x"}})), "item.color");
    expect_err(&edit(app, "document", "swatches", "add", json!({"item": {"color": "teal"}})), "item.color");
    expect_err(&edit(app, "document", "swatches", "add", json!({"item": {"color": "#fff", "hue": 1}})), "unknown item field");
    expect_err(&edit(app, "document", "swatches", "set", json!({"index": 0, "item": {}})), "set needs");
    expect_err(&edit(app, "document", "swatches", "set", json!({"index": 0, "item": {"name": " "}})), "non-empty");
    expect_err(&edit(app, "document", "swatches", "set", json!({"index": 9999, "item": {"name": "x"}})), "out of range");
    expect_err(&edit(app, "document", "swatches", "set", json!({"item": {"id": 424242, "name": "x"}})), "no swatch with id");
    expect_err(&edit(app, "document", "swatches", "move", json!({"index": 0})), "folder");
    expect_err(&edit(app, "document", "swatches", "remove", json!({})), "needs 'index'");
    expect_err(&edit(app, "document", "swatch_folders", "remove", json!({"item": {"name": "Nope"}})), "no folder");
    expect_err(&edit(app, "document", "swatch_folders", "add", json!({"item": {}})), "item.name");
    expect_err(&edit(app, "scene", "swatches", "get", json!({})), "'document'");
    assert_eq!(undo_depth(app), depth);
    assert_eq!(items_of(&edit(app, "document", "swatches", "get", json!({}))), base);
}

#[test]
fn edit_list_rig_poses_bones_and_rest_pose() {
    let mut fx = fixture();
    let rig = format!("element:{}", fx.armature);
    let shape = format!("element:{}", fx.shape);
    let app = &mut fx.app;
    let bones = items_of(&edit(app, &rig, "bones", "get", json!({})));
    assert!(bones.len() >= 5, "a person has bones: {bones:?}");
    assert!(items_of(&edit(app, &rig, "poses", "get", json!({}))).is_empty());
    let depth = undo_depth(app);

    // The pose library: save, apply, delete.
    let r = edit(app, &rig, "poses", "add", json!({"item": {"name": "rest"}}));
    assert_eq!((items_of(&r).len(), steps(&r)), (1, 1), "{:?}", r.value);
    assert_eq!(items_of(&r)[0]["bones"], json!(bones.len()));
    let r = edit(app, &rig, "poses", "set", json!({"index": 0}));
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    let r = edit(app, &rig, "poses", "remove", json!({"index": 0}));
    assert_eq!((items_of(&r).len(), steps(&r)), (0, 1));

    // Mirror, reset, and adopting the pose as the rest pose.
    for op in ["mirror_pose", "reset_pose", "set_rest_pose"] {
        let r = edit(app, &rig, "rig", op, json!({}));
        assert_eq!(steps(&r), 1, "{op}: {:?}", r.value);
    }
    assert_eq!(undo_depth(app), depth + 6);
    undo_n(app, 6);
    assert_eq!(undo_depth(app), depth);

    // Reparent: a bone to the root, and a cycle is refused.
    let child = bones.iter().find(|b| b["parent"].is_number()).expect("a bone with a parent");
    let (child_index, parent_index) = (child["index"].as_u64().unwrap(), child["parent"].as_u64().unwrap());
    let r = edit(app, &rig, "bones", "move", json!({"index": child_index, "to": null}));
    assert_eq!(steps(&r), 1, "{:?}", r.value);
    assert!(items_of(&r)[child_index as usize]["parent"].is_null(), "now a root");
    undo_n(app, 1);
    assert_eq!(items_of(&edit(app, &rig, "bones", "get", json!({}))), bones, "undo restored the skeleton");
    let depth = undo_depth(app);
    expect_err(&edit(app, &rig, "bones", "move", json!({"index": parent_index, "to": child_index})), "cannot hang off");
    expect_err(&edit(app, &rig, "bones", "move", json!({"index": child_index, "to": child_index})), "cannot hang off");
    expect_err(&edit(app, &rig, "bones", "move", json!({"index": child_index})), "needs 'to'");
    expect_err(&edit(app, &rig, "bones", "move", json!({"index": child_index, "to": 999})), "bone index");
    assert_eq!(undo_depth(app), depth, "refused moves change nothing");

    // Delete a bone: the skeleton shrinks by one, and undo brings it back.
    let last = bones.len() as u64 - 1;
    let r = edit(app, &rig, "bones", "remove", json!({"index": last}));
    assert_eq!((items_of(&r).len(), steps(&r)), (bones.len() - 1, 1), "{:?}", r.value);
    undo_n(app, 1);
    assert_eq!(items_of(&edit(app, &rig, "bones", "get", json!({}))), bones);

    // Bad calls.
    expect_err(&edit(app, &rig, "bones", "remove", json!({"index": 9999})), "out of range");
    expect_err(&edit(app, &rig, "poses", "add", json!({"item": {"name": " "}})), "item.name");
    expect_err(&edit(app, &rig, "poses", "set", json!({"index": 0})), "empty");
    expect_err(&edit(app, &rig, "poses", "remove", json!({"index": 0})), "empty");
    expect_err(&edit(app, &shape, "poses", "get", json!({})), "not an armature");
    expect_err(&edit(app, "document", "bones", "get", json!({})), "armature");
    expect_err(&edit(app, &rig, "rig", "wobble", json!({})), "no op 'wobble'");
    assert_eq!(undo_depth(app), depth);
}

#[test]
fn light_placement_and_sound_start_are_properties() {
    let mut fx = fixture();
    let lamp = format!("light:{}", fx.lamp);
    let sound = fx.sound_frame.clone();
    let app = &mut fx.app;

    // light.placement: the stage layer the light sits directly above, or null for the top.
    assert_eq!(read(app, &lamp, "placement"), Some(Value::Null));
    let depth = undo_depth(app);
    expect_err(&set(app, &lamp, "placement", json!(99)), "between");
    expect_err(&set(app, &lamp, "placement", json!(1.5)), "stage-layer index");
    expect_err(&set(app, &lamp, "placement", Value::Null), "already sits there");
    assert_eq!(undo_depth(app), depth);
    let r = set(app, &lamp, "placement", json!(1));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["undo_steps_added"], json!(1));
    assert_eq!(r.value["undo_label"], json!("Move Light"));
    assert_eq!(read(app, &lamp, "placement"), Some(json!(1)));
    undo_n(app, 1);
    assert_eq!(read(app, &lamp, "placement"), Some(Value::Null));

    // frame.sound_start: moves the keyframe that carries the sound.
    let (_, rest) = sound.split_at("frame:".len());
    let layer = rest.rsplit_once(':').unwrap().0.to_string();
    assert_eq!(read(app, &sound, "sound_start"), Some(json!(0)));
    let depth = undo_depth(app);
    expect_err(&set(app, &sound, "sound_start", json!(2.5)), "whole frame");
    expect_err(&set(app, &sound, "sound_start", json!(0)), "already starts");
    expect_err(&set(app, &sound, "sound_start", json!(-3)), "between");
    assert_eq!(undo_depth(app), depth);
    let r = set(app, &sound, "sound_start", json!(5));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["undo_steps_added"], json!(1), "{:?}", r.value);
    let moved = format!("frame:{layer}:5");
    assert_eq!(read(app, &moved, "sound_start"), Some(json!(5)), "the sound now starts on frame 5");
    assert!(read(app, &sound, "sound_start").is_none(), "nothing carries the sound on frame 0 any more");
    undo_n(app, 1);
    assert_eq!(read(app, &sound, "sound_start"), Some(json!(0)), "undo moved it back");
}

// -- Stage H: wait_for_jobs, transcribe, ai_character_from_image ------------

/// Poll `wait_for_jobs` (a snapshot through the executor) until nothing runs.
fn wait_idle(app: &mut App, secs: u64) -> Value {
    let started = std::time::Instant::now();
    loop {
        let r = call(app, "wait_for_jobs", json!({}));
        assert!(r.ok, "{:?}", r.error);
        if r.value["running"] == json!(0) {
            return r.value;
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(secs), "jobs still running: {}", r.value);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn wait_for_jobs_through_the_executor_is_a_snapshot_and_validates_its_argument() {
    let mut app = App::new(GpuPreference::default());
    let none = call(&mut app, "wait_for_jobs", json!({}));
    assert!(none.ok, "{:?}", none.error);
    assert_eq!(none.value["jobs"], json!([]));
    assert_eq!(none.value["running"], json!(0));
    assert!(call(&mut app, "wait_for_jobs", json!({"timeout_s": 10})).ok);
    for bad in [json!({"timeout_s": 0}), json!({"timeout_s": 301}), json!({"timeout_s": "soon"}), json!({"seconds": 3})] {
        let r = call(&mut app, "wait_for_jobs", bad.clone());
        assert!(!r.ok && r.error.is_some(), "{bad}");
    }
    // Both are listed tools with a schema hint, and the catalog says "tool".
    let cat = call(&mut app, "inspect", json!({"what": "catalog"})).value;
    for tool in ["wait_for_jobs", "transcribe"] {
        let entry = cat.as_array().unwrap().iter().find(|e| e["id"] == json!(tool)).unwrap_or_else(|| panic!("{tool}"));
        assert_eq!(entry["how"], json!("tool"));
        assert!(entry["args"].as_str().unwrap().starts_with('{'));
    }
}

#[test]
fn transcribe_checks_its_arguments_before_it_starts_anything() {
    let mut app = App::new(GpuPreference::default());
    let say = |r: &AgentResult| r.error.clone().unwrap_or_default();
    assert!(say(&call(&mut app, "transcribe", json!({}))).contains("missing required args"));
    assert!(say(&call(&mut app, "transcribe", json!({"sound": "a", "path": "b.wav"}))).contains("exactly one"));
    assert!(say(&call(&mut app, "transcribe", json!({"sound": ""}))).contains("needs text"));
    assert!(say(&call(&mut app, "transcribe", json!({"clip": "a"}))).contains("unknown arg 'clip'"));
    assert!(say(&call(&mut app, "transcribe", json!({"path": "Z:/nowhere/none.wav"}))).contains("no audio file at"));
    let r = call(&mut app, "transcribe", json!({"sound": "No Such Sound"}));
    assert!(say(&r).contains("no sound called 'No Such Sound'"), "{r:?}");
    // Nothing was started, so there is nothing to wait for.
    assert_eq!(call(&mut app, "wait_for_jobs", json!({})).value["jobs"], json!([]));
}

/// With Whisper installed this runs it for real on a one-second tone (no network,
/// no download: the model is already on disk). Without one it checks the error
/// says where to get it. Either way the job plumbing is the same one the AI
/// panel's tasks use.
#[test]
fn transcribe_runs_as_a_job_or_says_which_model_to_download() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("tone.wav");
    write_wav(&wav, 440.0);
    let mut app = App::new(GpuPreference::default());
    let r = call(&mut app, "transcribe", json!({"path": wav.to_string_lossy()}));
    match buzz_ai::transcribe::installed_whisper() {
        Err(why) => {
            assert!(!r.ok);
            assert!(r.error.unwrap().contains("AI \u{25b8} Models"), "{why}");
        }
        Ok(model) => {
            assert!(r.ok, "{:?}", r.error);
            assert_eq!(r.value["started"], json!(true));
            let job = r.value["job"].as_u64().expect("a job id");
            assert!(r.value["note"].as_str().unwrap().contains("wait_for_jobs"));
            let done = wait_idle(&mut app, 240);
            let listed = done["jobs"].as_array().unwrap().iter().find(|j| j["id"] == json!(job)).unwrap();
            assert_eq!(listed["kind"], json!("transcribe"));
            assert_eq!(listed["status"], json!("done"), "{listed}");
            let result = &listed["result"];
            assert_eq!(result["model"], json!(model.label()));
            assert_eq!(result["speakers"], json!("not identified"));
            assert!(result["segments"].is_array() && result["language"].is_string());
            assert!(result["audio_seconds"].as_f64().unwrap() > 0.5);
        }
    }
}

#[test]
fn character_from_image_refuses_bad_input_and_reports_a_failed_job() {
    let mut app = App::new(GpuPreference::default());
    let depth = undo_depth(&mut app);
    expect_err(&cmda(&mut app, "ai_character_from_image", json!({})), "path");
    expect_err(&cmda(&mut app, "ai_character_from_image", json!({"path": "x.png", "colour": 1})), "unknown arg 'colour'");
    expect_err(&cmda(&mut app, "ai_character_from_image", json!({"path": "x.png", "rig": "sticks"})), "'rig' must be");
    // The flat form (`path` beside `name`) works, and `name` there is the command.
    let flat = call(&mut app, "command", json!({"name": "ai_character_from_image", "path": "Z:/nowhere/hero.png"}));
    expect_err(&flat, "no image file at");

    // A file that is not a picture: with the models installed the job starts and
    // fails when it cannot decode it; without them the refusal names the model.
    let dir = tempfile::tempdir().unwrap();
    let junk = dir.path().join("hero.png");
    std::fs::write(&junk, b"this is not a picture").unwrap();
    let r = cmda(&mut app, "ai_character_from_image", json!({"path": junk.to_string_lossy(), "character_name": "Hero"}));
    let missing = buzz_ui::ai_character_dialog::missing_character_models(|m| buzz_ai::models::status(m).is_ready());
    if missing.is_empty() {
        assert!(r.ok, "{:?}", r.error);
        assert_eq!(r.value["started"], json!(true));
        let job = r.value["job"].as_u64().unwrap();
        let done = wait_idle(&mut app, 60);
        let listed = done["jobs"].as_array().unwrap().iter().find(|j| j["id"] == json!(job)).unwrap();
        assert_eq!(listed["status"], json!("failed"), "{listed}");
        assert!(listed["error"].as_str().unwrap().contains("decode"), "{listed}");
    } else {
        let why = r.error.expect("refused");
        assert!(why.contains("not installed") && why.contains(missing[0]), "{why}");
    }
    assert_eq!(undo_depth(&mut app), depth, "a failed job changes nothing");
}

/// **The agent tool end to end**: a drawn character image in, a rigged
/// character on stage out, as one undo step. Needs the six ONNX models and
/// takes minutes, so it is `#[ignore]`d like the wizard's own end-to-end test.
#[test]
#[ignore = "needs ONNX models"]
fn character_from_image_runs_the_whole_wizard_as_one_job() {
    let image = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../buzz-ai/tests/data/pujariFront_0.png"));
    assert!(image.exists(), "missing fixture {image:?}");
    let mut app = App::new(GpuPreference::default());
    // The character is saved to Cast: keep that out of the user's own folder.
    let assets = tempfile::tempdir().unwrap();
    app.live_use_assets_at(assets.path());
    let depth = undo_depth(&mut app);
    let r = cmda(
        &mut app,
        "ai_character_from_image",
        json!({"path": image.to_string_lossy(), "character_name": "Pujari Agent", "rig": "bones"}),
    );
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.value["started"], json!(true));
    let done = wait_idle(&mut app, 600);
    let result = &done["jobs"][0]["result"];
    assert_eq!(done["jobs"][0]["status"], json!("done"), "{done}");
    assert_eq!(result["character"], json!("Pujari Agent"));
    assert!(result["armature_element"].is_u64(), "{result}");
    assert_eq!(result["rig_kind"], json!("bones"), "{result}");
    assert_eq!(result["rig_root_element"], result["armature_element"]);
    assert!(!result["symbols"].as_array().unwrap().is_empty());
    assert_eq!(result["undo_steps_added"], json!(1));
    assert_eq!(undo_depth(&mut app), depth + 1);
    // The rig is on stage and can be told to walk, which is the point of it.
    let rig = result["armature_element"].as_u64().unwrap();
    let walk = cmda(&mut app, "perform", json!({"action": "walk", "element": rig}));
    assert!(walk.ok, "{:?}", walk.error);
}

/// LR2: with no `rig` the job builds a layer rig — no armature, and a root
/// part to perform with. Needs the ONNX models, so `#[ignore]`d.
#[test]
#[ignore = "needs ONNX models"]
fn character_from_image_defaults_to_a_layer_rig() {
    let image = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../buzz-ai/tests/data/pujariFront_0.png"));
    assert!(image.exists(), "missing fixture {image:?}");
    let mut app = App::new(GpuPreference::default());
    let assets = tempfile::tempdir().unwrap();
    app.live_use_assets_at(assets.path());
    let r = cmda(&mut app, "ai_character_from_image", json!({"path": image.to_string_lossy(), "character_name": "Pujari Layers"}));
    assert!(r.ok, "{:?}", r.error);
    let done = wait_idle(&mut app, 600);
    let result = &done["jobs"][0]["result"];
    assert_eq!(done["jobs"][0]["status"], json!("done"), "{done}");
    assert_eq!(result["rig_kind"], json!("layers"), "{result}");
    assert!(result["armature_element"].is_null(), "{result}");
    let root = result["rig_root_element"].as_u64().expect("a numeric rig_root_element");
    let walk = cmda(&mut app, "perform", json!({"action": "walk", "element": root}));
    assert!(walk.ok, "{:?}", walk.error);
}
