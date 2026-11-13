//! The Assistant's `arrange_assets` (AS2): Arrange by type & style from the
//! asset index, as a dry run (the default) or applied with a move log that
//! the Assets panel's Undo arrange reverses.

use serde_json::{Value, json};

use super::AgentResult;
use crate::app::App;

/// Moves listed in a dry run's answer (the counts cover all of them).
const LISTED: usize = 100;

/// The Assistant's `dress_set_from_assets {query, style?}` (AS3): dress the
/// open scene's set from the asset library for the words in `query` (a
/// slugline and what happens: "EXT. CHHOTU'S TEA STALL — MORNING, a village
/// lane"), in the style of the asset `style` names when given. The pieces
/// go behind everything already there (in front of the `Sky`/`Ground` layers
/// Set the scene makes, when it is there), clear of the rigs on stage; one
/// undo step. Returns every piece with why it was chosen.
pub fn dress_set_from_assets(app: &mut App, args: &Value) -> AgentResult {
    let Some(map) = args.as_object() else {
        return AgentResult::err("args is an object: {query, style?}");
    };
    if let Some(key) = map.keys().find(|k| !matches!(k.as_str(), "query" | "style")) {
        return AgentResult::err(format!("unknown arg '{key}'; valid: query, style"));
    }
    let query = match map.get("query").and_then(Value::as_str).map(str::trim) {
        Some(q) if !q.is_empty() => q.to_string(),
        _ => return AgentResult::err("missing required args: query (the scene's words)"),
    };
    let style = match map.get("style").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => match v.as_str() {
            Some(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
            None => return AgentResult::err("'style' is the name of an asset whose style to follow"),
        },
    };
    let Some(index) = crate::asset_sets::index_for(&app.agent_editor().assets) else {
        return AgentResult::err(
            "the assets are not indexed yet: the Assets panel's 'Arrange by type & style…' builds the index (a Tasks-panel job)",
        );
    };
    let lower = format!(" {} ", query.to_lowercase());
    let interior = lower.trim_start().starts_with("int.")
        || [" interior", " inside", " indoor", " room", " kamra"].iter().any(|w| lower.contains(w));
    let time = if [" night", " raat"].iter().any(|w| lower.contains(w)) {
        buzz_ai::asset_sets::TimeOfDay::Night
    } else if [" evening", " sunset", " dusk", " shaam"].iter().any(|w| lower.contains(w)) {
        buzz_ai::asset_sets::TimeOfDay::Evening
    } else {
        buzz_ai::asset_sets::TimeOfDay::Day
    };
    let before = app.agent_editor().doc.history().undo_depth();
    let editor = app.agent_editor_mut();
    editor.doc.end_gesture();
    let horizon = editor.story.horizon.clamp(0.15, 0.95);
    let frame_no = editor.current_frame;
    let assets = &editor.assets;
    let mut dressed = None;
    let mut staged_here = false;
    editor.doc.edit("Dress Set from Assets", |scene| {
        // Nothing to stand on yet: a ground and a sky first (Set the scene
        // with no cast), so the pieces have a ground line under them.
        if crate::asset_sets::SetFrame::by_name(scene).ground.is_none() {
            let setting = if interior {
                buzz_act::Setting::Interior
            } else {
                match time {
                    buzz_ai::asset_sets::TimeOfDay::Night => buzz_act::Setting::Night,
                    buzz_ai::asset_sets::TimeOfDay::Evening => buzz_act::Setting::Sunset,
                    buzz_ai::asset_sets::TimeOfDay::Day => buzz_act::Setting::Daylight,
                }
            };
            let recipe = buzz_act::SceneRecipe {
                setting,
                cast: 0,
                horizon,
                frames: scene.frame_count().max(1),
                ..buzz_act::SceneRecipe::default()
            };
            buzz_act::stage_scene(scene, &recipe);
            staged_here = true;
        }
        let request = buzz_ai::asset_sets::SetQuery {
            text: query.clone(),
            interior,
            time,
            style: style.clone(),
            seed: buzz_ai::asset_sets::seed_of(&query),
            marks: crate::asset_sets::rig_marks(scene, frame_no),
            horizon,
            ground: horizon + (1.0 - horizon) * 0.72,
        };
        let frame = crate::asset_sets::SetFrame::by_name(scene);
        dressed = Some(crate::asset_sets::dress_set(scene, assets, &index, &request, &frame));
    });
    editor.doc.end_gesture();
    let Some(dressed) = dressed else {
        return AgentResult::err("the document could not be edited");
    };
    if dressed.layers.is_empty() {
        if editor.doc.history().undo_depth() > before {
            editor.doc.undo();
        }
        return AgentResult::err(format!(
            "nothing in the assets fits {query:?}: {}",
            if dressed.plan.notes.is_empty() { "no piece matched".to_string() } else { dressed.plan.notes.join("; ") }
        ));
    }
    editor.status = Some(format!("Set dressed from your assets: {}", dressed.summary()));
    let pieces: Vec<Value> = dressed
        .plan
        .pieces
        .iter()
        .filter(|p| !dressed.failed.iter().any(|f| f.starts_with(&p.id)))
        .map(|p| json!({"role": p.role.label(), "name": p.name, "record": p.id, "why": p.why}))
        .collect();
    let after = editor.doc.history().undo_depth();
    AgentResult::ok(json!({
        "pieces": pieces,
        "notes": dressed.plan.notes,
        "failed": dressed.failed,
        "interior": interior,
        "staged_ground_and_sky": staged_here,
        "procedural_advised": dressed.needs_procedural(),
        "layers": dressed.layers.len(),
        "undo_steps_added": after as i64 - before as i64,
    }))
}

pub fn arrange_assets(app: &mut App, args: &Value) -> AgentResult {
    if let Some(map) = args.as_object() {
        if let Some(key) = map.keys().find(|k| k.as_str() != "dry_run") {
            return AgentResult::err(format!("unknown arg '{key}'; valid: dry_run"));
        }
    } else if !args.is_null() {
        return AgentResult::err("args is an object");
    }
    let dry_run = match args.get("dry_run").filter(|v| !v.is_null()) {
        None => true,
        Some(v) => match v.as_bool() {
            Some(b) => b,
            None => return AgentResult::err("'dry_run' is true or false"),
        },
    };
    let library = &app.agent_editor().assets;
    let Some(root) = library.root().map(std::path::Path::to_path_buf) else {
        return AgentResult::err("there is no assets folder on this machine");
    };
    let index = buzz_ai::asset_index::AssetIndex::load(&root);
    if index.records.is_empty() {
        return AgentResult::err(
            "the assets are not indexed yet: the Assets panel's 'Arrange by type & style…' builds the index (a Tasks-panel job)",
        );
    }
    let plan = crate::asset_index::plan_for(library);
    let counts = plan.counts();
    if dry_run {
        let moves: Vec<Value> = plan
            .moves
            .iter()
            .take(LISTED)
            .map(|m| json!({"name": m.name, "from": m.from, "to": m.to, "guessed": m.guessed}))
            .collect();
        return AgentResult::ok(json!({
            "dry_run": true,
            "moves": plan.moves.len(),
            "counts": counts,
            "cast_kept": plan.cast_kept,
            "unindexed": plan.unindexed,
            "indexed_records": index.records.len(),
            "guessed_records": index.records.iter().filter(|r| r.guessed).count(),
            "first_moves": moves,
            "note": "nothing was moved; call with dry_run:false to apply (the Assets panel's Undo arrange reverses it)",
        }));
    }
    let editor = app.agent_editor_mut();
    let result = buzz_ai::asset_index::apply_arrange(&mut editor.assets, &plan.moves, |_, _| {});
    app.agent_assets_moved();
    match result {
        Ok(log) => AgentResult::ok(json!({
            "dry_run": false,
            "moved": log.moved.len(),
            "created_folders": log.created_folders,
            "counts": counts,
            "undo": "Assets panel ▸ Arrange by type & style… ▸ Undo arrange",
        })),
        Err(e) => AgentResult::err(format!("could not arrange the assets: {e}")),
    }
}

/// The Assistant's `cast_start_fresh {name | all: true, keep_frame?}` (CAST_FRESH
/// F1): clear every keyframe of a saved Cast character (or all of them),
/// keeping one pose, with a backup in `_cast_backup/<stamp>/` first.
pub fn cast_start_fresh(app: &mut App, args: &Value) -> AgentResult {
    let Some(map) = args.as_object() else {
        return AgentResult::err("args is an object: {name} or {all: true}");
    };
    if let Some(key) = map.keys().find(|k| !matches!(k.as_str(), "name" | "all" | "keep_frame")) {
        return AgentResult::err(format!("unknown arg '{key}'; valid: name, all, keep_frame"));
    }
    let all = match map.get("all").filter(|v| !v.is_null()) {
        None => false,
        Some(v) => match v.as_bool() {
            Some(b) => b,
            None => return AgentResult::err("'all' is true or false"),
        },
    };
    let keep = match map.get("keep_frame").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => match v.as_u64() {
            Some(f) => Some(f as u32),
            None => return AgentResult::err("'keep_frame' is a frame number"),
        },
    };
    let name = map.get("name").and_then(Value::as_str);
    let asset = match (all, name) {
        (true, None) => None,
        (false, Some(name)) => {
            let cast = crate::cast_fresh::cast_characters(&app.agent_editor().assets);
            let wanted = name.trim().to_lowercase();
            match cast.into_iter().find(|a| a.name.to_lowercase() == wanted || a.label().to_lowercase() == wanted) {
                Some(a) => Some(a),
                None => return AgentResult::err(format!("no character called '{name}' in the Cast")),
            }
        }
        _ => return AgentResult::err("give either {name} or {all: true}"),
    };
    match app.start_fresh_cast(asset, keep) {
        Ok(done) => {
            let items: Vec<Value> = done
                .iter()
                .map(|o| {
                    json!({
                        "name": o.name,
                        "backup": o.backup.display().to_string(),
                        "symbols": o.report.symbols,
                        "keyframes_cleared": o.report.keyframes_cleared + o.report.stage_keyframes_cleared,
                        "picker_symbols_left": o.report.skipped,
                    })
                })
                .collect();
            AgentResult::ok(json!({"cleared": items.len(), "characters": items}))
        }
        Err(e) => AgentResult::err(e),
    }
}

/// The Assistant's `cast_delete {names | all_listed}` (CAST_POSES C1): move Cast
/// assets to `_cast_backup/<stamp>_deleted/`; never erased.
pub fn cast_delete(app: &mut App, args: &Value) -> AgentResult {
    let Some(map) = args.as_object() else {
        return AgentResult::err("args is an object: {names} or {all_listed: true}");
    };
    if let Some(key) = map.keys().find(|k| !matches!(k.as_str(), "names" | "all_listed")) {
        return AgentResult::err(format!("unknown arg '{key}'; valid: names, all_listed"));
    }
    let all_listed = map.get("all_listed").and_then(Value::as_bool).unwrap_or(false);
    let editor = app.agent_editor();
    let cast = crate::cast_fresh::cast_characters(&editor.assets);
    let targets: Vec<buzz_doc::Asset> = if all_listed {
        let visible: Vec<std::path::PathBuf> = editor.cast_panel.visible(&cast).iter().map(|a| a.path.clone()).collect();
        cast.into_iter().filter(|a| visible.contains(&a.path)).collect()
    } else if let Some(names) = map.get("names").and_then(Value::as_array) {
        let mut out = Vec::new();
        for name in names {
            let Some(name) = name.as_str() else {
                return AgentResult::err("'names' is a list of Cast character names");
            };
            let wanted = name.trim().to_lowercase();
            match cast.iter().find(|a| a.name.to_lowercase() == wanted || a.label().to_lowercase() == wanted) {
                Some(a) => out.push(a.clone()),
                None => return AgentResult::err(format!("no character called '{name}' in the Cast")),
            }
        }
        out
    } else {
        return AgentResult::err("give {names: [...]} or {all_listed: true}");
    };
    let root = editor.assets.root().map(std::path::Path::to_path_buf).unwrap_or_default();
    let in_series = crate::cast_fresh::cast_in_series(&root, &targets);
    match app.delete_cast_many(&targets) {
        Ok(done) => AgentResult::ok(json!({
            "moved": done.len(),
            "backup": done.first().and_then(|d| d.backup.parent()).map(|p| p.display().to_string()),
            "cast_in_series": in_series,
        })),
        Err(e) => AgentResult::err(e),
    }
}

/// The Assistant's `cast_combine_poses {names?}` (CAST_POSES C2b): merge each
/// role's separate view/pose assets into one Cast asset whose symbol set holds
/// every pose. The separate files are moved to `_cast_backup/<stamp>_combine/`.
pub fn cast_combine_poses(app: &mut App, args: &Value) -> AgentResult {
    let Some(map) = args.as_object() else {
        return AgentResult::err("args is an object: {names?: [role names]}");
    };
    if let Some(key) = map.keys().find(|k| k.as_str() != "names") {
        return AgentResult::err(format!("unknown arg '{key}'; valid: names"));
    }
    let names: Option<Vec<String>> = map
        .get("names")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
    let stamp = crate::cast_fresh::backup_stamp();
    let editor = app.agent_editor_mut();
    let (done, failed) = crate::cast_combine::combine_many(&mut editor.assets, names.as_deref(), &stamp);
    app.agent_assets_moved();
    if done.is_empty() {
        return AgentResult::err(if failed.is_empty() { "nothing to combine".to_string() } else { failed.join("; ") });
    }
    let combined: Vec<Value> = done.iter().map(|r| json!({"role": r.role, "poses": r.poses})).collect();
    AgentResult::ok(json!({
        "combined": combined,
        "backup": done[0].backup.display().to_string(),
        "failed": failed,
    }))
}

/// The Assistant's `import_cast_fla {fla, series, ai_rig?, roles?}` (ALAXMI plan B1a): the
/// cast FLA's roles filed as locked Cast assets under `Cast/<series>/` and the
/// series' `casting.json` written; the Cast folder is backed up first. A job.
pub fn import_cast_fla(app: &mut App, args: &Value) -> AgentResult {
    let Some(map) = args.as_object() else {
        return AgentResult::err("args is an object: {fla, series, ai_rig?, roles?}");
    };
    if let Some(key) = map.keys().find(|k| !matches!(k.as_str(), "fla" | "series" | "ai_rig" | "roles")) {
        return AgentResult::err(format!("unknown arg '{key}'; valid: fla, series, ai_rig, roles"));
    }
    let text = |key: &str| map.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let (Some(fla), Some(series)) = (text("fla"), text("series")) else {
        return AgentResult::err("missing required args: fla, series");
    };
    // The folder, not a scan of it (`AssetLibrary::user` opens every asset).
    let root = buzz_doc::assets::default_root();
    let only: Vec<String> = map.get("roles").and_then(Value::as_array).map(|a| a.iter().filter_map(|r| r.as_str().map(String::from)).collect()).unwrap_or_default();
    let ai_rig = map.get("ai_rig").and_then(Value::as_bool).unwrap_or(true);
    let req = crate::cast_fla::CastFlaRequest { fla: std::path::PathBuf::from(fla), root, series, ai_rig, only };
    match app.agent_cast_fla_async(req) {
        Ok(started) => AgentResult::ok(started),
        Err(e) => AgentResult::err(e),
    }
}
