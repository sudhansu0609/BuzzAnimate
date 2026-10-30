//! scratch probe — delete after use. The Storyboard panel (STORYBOARD_PLAN S3)
//! driven headlessly through the agent's storyboard actions, then drawn to PNGs.
//!
//! zz_storyboard_panel_probe <episode dir> <breakdown.json> <out dir> [<doc.buzz>]
use std::path::PathBuf;

use buzz_app::agent::{AgentCall, execute};
use buzz_app::app::App;
use buzz_app::ui_snapshot::{SnapshotConfig, write_frame, write_ui_snapshot};
use buzz_render::GpuPreference;
use serde_json::{Value, json};

fn call(app: &mut App, name: &str, mut args: Value) -> Value {
    args["name"] = json!(name);
    let t = std::time::Instant::now();
    let r = execute(app, &AgentCall { tool: "command".into(), args });
    println!("== {name}: ok={} ({:.1}s){}", r.ok, t.elapsed().as_secs_f64(), r.error.as_ref().map(|e| format!(" error: {e}")).unwrap_or_default());
    r.value
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    unsafe {
        std::env::set_var("BUZZANIMATE_WORKSPACE", std::env::temp_dir().join(format!("buzz-sb-probe-{}.json", std::process::id())));
    }
    let (episode, breakdown, out) = (&a[0], &a[1], PathBuf::from(&a[2]));
    let board_dir = out.join("app_board");
    let mut app = App::new(GpuPreference::default());

    let made = call(&mut app, "storyboard_make", json!({"episode": episode, "scenes": "4,5", "out": board_dir, "look": "a", "breakdown": breakdown}));
    let panels = made["panels"].as_array().cloned().unwrap_or_default();
    println!("board {} | {} panels | status {}", made["board"], panels.len(), made["status"]);
    for p in panels.iter().take(3) {
        println!("  {} {} on {:?}: {} | {}", p["id"], p["kind"], p["on"], p["look_line"], p["framing"]);
    }

    // Edits, each straight into storyboard.json.
    let e = call(&mut app, "storyboard_edit", json!({"panel": 1, "notes": "hold on Guddu's hand", "action": "Guddu points at the road.", "duration_s": 3.0}));
    println!("  edited: {} notes={:?}", e["panel"]["id"], e["panel"]["notes"]);
    let e = call(&mut app, "storyboard_edit", json!({"panel": 3, "move": "up"}));
    println!("  moved to {}: {}", e["index"], e["panel"]["id"]);
    let e = call(&mut app, "storyboard_edit", json!({"panel": "s4-sh7-l10-shot", "split": true}));
    println!("  split {} -> {} panels", e["panel"]["id"], e["panels"].as_array().map_or(0, |v| v.len()));
    let e = call(&mut app, "storyboard_edit", json!({"panel": 0, "add_after": true}));
    println!("  added {} at {}", e["panel"]["id"], e["index"]);
    let added = e["panel"]["id"].as_str().unwrap_or("").to_string();
    let e = call(&mut app, "storyboard_edit", json!({"panel": added, "delete": true}));
    println!("  deleted user panel -> {} panels", e["panels"].as_array().map_or(0, |v| v.len()));
    let e = call(&mut app, "storyboard_edit", json!({"panel": "s4-sh8-l13-shot", "delete": true}));
    println!("  deleted made panel -> {} panels", e["panels"].as_array().map_or(0, |v| v.len()));
    let e = call(&mut app, "storyboard_edit", json!({"panel": "s5-sh3-l4-shot", "kind": "close", "regenerate": true}));
    let row = e["panels"].as_array().and_then(|v| v.iter().find(|p| p["id"] == "s5-sh3-l4-shot").cloned()).unwrap_or(Value::Null);
    println!("  regenerated s5-sh3-l4-shot as {}: {} | {}", e["panel"]["kind"], row["framing"], row["look_line"]);

    let board_json = board_dir.join("storyboard.json");
    let disk: Value = serde_json::from_str(&std::fs::read_to_string(&board_json).unwrap()).unwrap();
    println!(
        "on disk: {} panels, removed {}, panel 1 notes {:?}, ids {:?}",
        disk["panels"].as_array().unwrap().len(),
        disk["removed"],
        disk["panels"][1]["notes"],
        disk["panels"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>()
    );

    let x = call(&mut app, "storyboard_export", json!({}));
    println!("export: pages {} pdf {} warnings {}", x["pages"], x["pdf"], x["warnings"]);

    // Make again: the edits must survive the merge.
    let again = call(&mut app, "storyboard_make", json!({"episode": episode, "scenes": "4,5", "out": board_dir, "look": "a", "breakdown": breakdown}));
    let disk2: Value = serde_json::from_str(&std::fs::read_to_string(&board_json).unwrap()).unwrap();
    let ids2: Vec<String> = disk2["panels"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap_or("").to_string()).collect();
    let p1 = disk2["panels"].as_array().unwrap().iter().find(|p| p["id"] == disk["panels"][1]["id"]).cloned().unwrap_or(Value::Null);
    println!(
        "after re-make: {} panels ({}), deleted made panel back? {}, panel {} notes {:?} duration {}, close kept: {}, ids {:?}",
        ids2.len(),
        again["status"],
        ids2.iter().any(|i| i == "s4-sh8-l13-shot"),
        p1["id"],
        p1["notes"],
        p1["duration_s"],
        disk2["panels"].as_array().unwrap().iter().any(|p| p["id"] == "s5-sh3-l4-shot" && p["kind"] == "close"),
        ids2
    );

    // Open scene, against the real episode document when given.
    if let Some(doc) = a.get(3) {
        let r = call(&mut app, "open", json!({"path": doc}));
        let _ = r;
        // The document is read in the background.
        let t = std::time::Instant::now();
        loop {
            let _ = execute(&mut app, &AgentCall { tool: "wait_for_jobs".into(), args: json!({}) });
            match app.storyboard_open_scene(5, 325) {
                Ok(name) => {
                    let doc = execute(&mut app, &AgentCall { tool: "inspect".into(), args: json!({"what": "document"}) });
                    println!("open scene: {name} at frame 325 after {:.1}s; inspect: scene {} frame {}", t.elapsed().as_secs_f64(), doc.value["scene"], doc.value["frame"]);
                    break;
                }
                Err(e) if t.elapsed().as_secs() > 180 => {
                    println!("open scene: {e}");
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(250)),
            }
        }
    }

    // Pictures of the panel.
    app.reveal_storyboard();
    app.storyboard_mut().selected = 1;
    app.storyboard_mut().forget_textures();
    let png = out.join("storyboard_panel.png");
    write_frame(SnapshotConfig { width: 1500, height: 900, scale: 1.0 }, &png, |ui| {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let _ = app.storyboard_mut().ui(ui);
        });
    })
    .expect("panel png");
    println!("panel png {}", png.display());
    app.storyboard_mut().forget_textures();
    let full = out.join("storyboard_window.png");
    write_ui_snapshot(&mut app, SnapshotConfig { width: 1600, height: 900, scale: 1.0 }, &full).expect("window png");
    println!("window png {}", full.display());
}
