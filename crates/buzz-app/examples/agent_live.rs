//! Stage G of docs/AI_AGENT_PLAN.md: live acceptance of the Assistant.
//!
//! Drives a headless `App` through the real Assistant path (the same backend
//! selection, tool loop, confirmation and undo-collapse as the AI panel's Send
//! button) against whatever backend the saved AI settings select, runs the six
//! acceptance briefs, checks each result in code and prints a pass/fail table.
//!
//! ```text
//! cargo run --release -p buzz-app --example agent_live -- [--brief N] [--out DIR]
//!     [--auto-confirm yes|no] [--timeout-secs S]
//! ```
//!
//! Exit codes: 0 all passed, 1 some failed, 2 no backend available.
//! The log never contains the API key or request headers: only the panel's
//! step log (tool call summaries, results, reply) and the backend label.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use buzz_app::agent::{self, AgentCall};
use buzz_app::app::App;
use buzz_render::GpuPreference;
use serde_json::{Value, json};

const BRIEF_1: &str = "New layer called Ball, draw a red circle at the left, make it bounce to the right over 48 frames with a classic tween.";
const BRIEF_2: &str = "Paint the circle blue with the bucket.";
const BRIEF_3: &str = "Add a sun and a lamp, key the lamp brighter at frame 24.";
const BRIEF_5: &str = "Undo everything you just did.";

fn brief_4(wav: &Path) -> String {
    format!(
        "Import {} and lip-sync the character. If there is no character yet, make a simple placeholder mouth.",
        wav.display()
    )
}

// ------------------------------------------------------------------- checks

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() < 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some((p(0)?, p(2)?, p(4)?))
}

fn is_reddish(hex: &str) -> bool {
    parse_hex(hex).is_some_and(|(r, g, b)| r >= 150 && (g as i32) < r as i32 - 70 && (b as i32) < r as i32 - 70)
}

fn is_bluish(hex: &str) -> bool {
    parse_hex(hex).is_some_and(|(r, g, b)| b >= 120 && (b as i32) > r as i32 + 40 && (b as i32) > g as i32 + 20)
}

fn layer<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    state["layers"].as_array()?.iter().find(|l| l["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

fn keyframes(layer: &Value) -> Vec<&Value> {
    layer["keyframes"].as_array().map(|k| k.iter().collect()).unwrap_or_default()
}

/// The ovals (with `fill`, `x`) of one keyframe.
fn ovals(key: &Value) -> Vec<&Value> {
    key["objects"].as_array().map(|o| o.iter().filter(|o| o["oval"] == json!(true)).collect()).unwrap_or_default()
}

fn depth(state: &Value) -> i64 {
    state["undo_depth"].as_i64().unwrap_or(-1)
}

fn one_step(before: &Value, after: &Value, fails: &mut Vec<String>) {
    if depth(after) != depth(before) + 1 {
        fails.push(format!("run is not exactly one undo step (depth {} -> {})", depth(before), depth(after)));
    } else if !after["next_undo_label"].as_str().is_some_and(|l| l.starts_with("AI:")) {
        fails.push(format!("undo step is not labelled 'AI:' ({})", after["next_undo_label"]));
    }
}

/// Brief 1: layer Ball, a red oval, keyframes at 1 and 48, classic tween, x increases.
fn check_brief1(before: &Value, after: &Value) -> Vec<String> {
    let mut f = Vec::new();
    let Some(ball) = layer(after, "Ball") else {
        return vec!["no layer named Ball".into()];
    };
    let keys = keyframes(ball);
    let starts: Vec<i64> = keys.iter().filter_map(|k| k["start"].as_i64()).collect();
    let has = |a: i64, b: i64| starts.contains(&a) && starts.contains(&b);
    if !(has(0, 47) || has(1, 48)) {
        f.push(format!("keyframes at {starts:?}, wanted frame 1 and 48 (0-based 0 and 47)"));
    }
    let Some(first) = keys.iter().min_by_key(|k| k["start"].as_i64().unwrap_or(i64::MAX)) else {
        f.push("Ball has no keyframes".into());
        return f;
    };
    let last = keys.iter().max_by_key(|k| k["start"].as_i64().unwrap_or(i64::MIN)).unwrap();
    if first["tween"].as_str() != Some("Classic") {
        f.push(format!("first keyframe tween is {}, wanted Classic", first["tween"]));
    }
    let o1 = ovals(first);
    if o1.is_empty() {
        f.push("no oval in the first keyframe".into());
    } else if !o1.iter().any(|o| o["fill"].as_str().is_some_and(is_reddish)) {
        f.push(format!("no red-ish oval fill (fills: {:?})", o1.iter().map(|o| &o["fill"]).collect::<Vec<_>>()));
    }
    let (o2, x1) = (ovals(last), o1.first().and_then(|o| o["x"].as_f64()));
    match (x1, o2.first().and_then(|o| o["x"].as_f64())) {
        (Some(a), Some(b)) if b > a + 1.0 => {}
        (a, b) => f.push(format!("x does not increase ({a:?} -> {b:?})")),
    }
    one_step(before, after, &mut f);
    f
}

/// Brief 2: the Ball's oval is blue, no new shapes, the bucket was used.
fn check_brief2(before: &Value, after: &Value, bucket_used: bool) -> Vec<String> {
    let mut f = Vec::new();
    match layer(after, "Ball") {
        None => f.push("no layer named Ball".into()),
        Some(ball) => {
            let blue = keyframes(ball)
                .iter()
                .any(|k| ovals(k).iter().any(|o| o["fill"].as_str().is_some_and(is_bluish)));
            if !blue {
                f.push("no blue-ish oval fill on Ball".into());
            }
        }
    }
    if after["shapes"] != before["shapes"] {
        f.push(format!("shape count changed {} -> {}", before["shapes"], after["shapes"]));
    }
    if !bucket_used {
        f.push("the PaintBucket gesture was not used".into());
    }
    one_step(before, after, &mut f);
    f
}

/// Brief 3: a sun and a lamp (2 more lights), the lamp has a brighter key at frame 24.
fn check_brief3(before: &Value, after: &Value) -> Vec<String> {
    let mut f = Vec::new();
    let lights = after["lights"].as_array().cloned().unwrap_or_default();
    let n0 = before["lights"].as_array().map(|l| l.len()).unwrap_or(0);
    if lights.len() < n0 + 2 {
        f.push(format!("lights {} -> {}, wanted 2 more", n0, lights.len()));
    }
    let kinds: Vec<&str> = lights.iter().filter_map(|l| l["kind"].as_str()).collect();
    if !kinds.contains(&"sun") {
        f.push(format!("no sun among {kinds:?}"));
    }
    let mut keyed = false;
    for l in lights.iter().filter(|l| l["kind"] == json!("lamp")) {
        let keys = l["keys"].as_array().cloned().unwrap_or_default();
        let base = l["intensity"].as_f64().unwrap_or(0.0);
        // Frame 24 as the model may count it: 1-based 24 is 0-based 23.
        for k in keys.iter().filter(|k| matches!(k["frame"].as_i64(), Some(23) | Some(24))) {
            let v = k["intensity"].as_f64().unwrap_or(0.0);
            let other_lower = keys.iter().any(|o| o["frame"] != k["frame"] && o["intensity"].as_f64().is_some_and(|x| x < v - 1e-6));
            if v > base + 1e-6 || other_lower {
                keyed = true;
            }
        }
    }
    if !lights.iter().any(|l| l["kind"] == json!("lamp")) {
        f.push("no lamp".into());
    } else if !keyed {
        f.push("no brighter light key at frame 24 on a lamp".into());
    }
    one_step(before, after, &mut f);
    f
}

fn instance_keys(state: &Value) -> usize {
    state["layers"]
        .as_array()
        .map(|ls| {
            ls.iter()
                .flat_map(|l| keyframes(l))
                .filter(|k| k["instances"].as_u64().unwrap_or(0) > 0)
                .count()
        })
        .unwrap_or(0)
}

/// Brief 4: the sound is in the document and attached to a keyframe, and mouths got keys.
fn check_brief4(before: &Value, after: &Value) -> Vec<String> {
    let mut f = Vec::new();
    if after["sounds"].as_u64().unwrap_or(0) <= before["sounds"].as_u64().unwrap_or(0) {
        f.push("no sound was imported".into());
    }
    let attached = after["layers"]
        .as_array()
        .is_some_and(|ls| ls.iter().any(|l| keyframes(l).iter().any(|k| k["sound"] == json!(true))));
    if !attached {
        f.push("no keyframe carries the sound".into());
    }
    let (b, a) = (instance_keys(before), instance_keys(after));
    if a < b + 2 {
        f.push(format!("mouth keys: instance keyframes {b} -> {a}, wanted at least 2 more"));
    }
    one_step(before, after, &mut f);
    f
}

/// What undo must bring back: the document's shape, not its undo stack.
fn summary(state: &Value) -> Value {
    let layers: Vec<Value> = state["layers"]
        .as_array()
        .map(|ls| ls.iter().map(|l| json!({"name": l["name"], "keys": keyframes(l).len()})).collect())
        .unwrap_or_default();
    json!({
        "layers": layers,
        "shapes": state["shapes"],
        "lights": state["lights"].as_array().map(|l| l.len()),
        "sounds": state["sounds"],
    })
}

/// Brief 5: after "undo everything", the document is as before brief 1 (one undo per run).
fn check_brief5(baseline: &Value, after: &Value) -> Vec<String> {
    let mut f = Vec::new();
    if summary(baseline) != summary(after) {
        f.push(format!("document not restored: before {} after {}", summary(baseline), summary(after)));
    }
    if depth(after) > depth(baseline) + 1 {
        f.push(format!("undo stack deeper than baseline + the run's own step ({} vs {})", depth(after), depth(baseline)));
    }
    f
}

/// Brief 6: stopped mid-run: consistent doc, exactly one `AI:` step, one undo restores.
fn check_brief6(baseline: &Value, after: &Value, after_undo: &Value, running: bool, undo_ok: bool) -> Vec<String> {
    let mut f = Vec::new();
    if running {
        f.push("run still active after stop".into());
    }
    if summary(baseline) != summary(after) {
        one_step(baseline, after, &mut f);
    } else if depth(after) != depth(baseline) && depth(after) != depth(baseline) + 1 {
        f.push("inconsistent undo depth after stop".into());
    }
    if !undo_ok {
        f.push("undo failed after the stopped run".into());
    }
    if summary(baseline) != summary(after_undo) {
        f.push("one undo did not restore the pre-run document".into());
    }
    f
}

// ----------------------------------------------------------------- the run

struct Opts {
    brief: Option<u32>,
    out: PathBuf,
    auto_confirm: bool,
    timeout: Duration,
}

fn parse_args() -> Result<Opts, String> {
    let mut o = Opts {
        brief: None,
        out: PathBuf::from("agent_live_out"),
        auto_confirm: false,
        timeout: Duration::from_secs(300),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--brief" => {
                let n: u32 = val("--brief")?.parse().map_err(|_| "--brief needs 1..6".to_string())?;
                if !(1..=6).contains(&n) {
                    return Err("--brief needs 1..6".into());
                }
                o.brief = Some(n);
            }
            "--out" => o.out = PathBuf::from(val("--out")?),
            "--auto-confirm" => {
                o.auto_confirm = match val("--auto-confirm")?.as_str() {
                    "yes" => true,
                    "no" => false,
                    _ => return Err("--auto-confirm needs yes|no".into()),
                }
            }
            "--timeout-secs" => {
                let s: u64 = val("--timeout-secs")?.parse().map_err(|_| "--timeout-secs needs a number".to_string())?;
                o.timeout = Duration::from_secs(s);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(o)
}

/// A 1.5 s mono tone with amplitude bursts, so lip sync finds mouth shapes.
fn write_wav(path: &Path) -> Result<(), String> {
    let spec = hound::WavSpec { channels: 1, sample_rate: 22050, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    let n = (22050.0 * 1.5) as usize;
    for i in 0..n {
        let t = i as f32 / 22050.0;
        // 0.25 s on, 0.1 s off.
        let env = if (t % 0.35) < 0.25 { 0.6 } else { 0.0 };
        let s = (t * 220.0 * std::f32::consts::TAU).sin() * env;
        w.write_sample((s * i16::MAX as f32) as i16).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

struct RunInfo {
    secs: f64,
    calls: usize,
    turns: usize,
    timed_out: bool,
    events: Vec<Value>,
    bucket_used: bool,
    reply: String,
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// Send `request` and pump until the run ends (or time runs out), logging events.
fn run_brief(app: &mut App, n: u32, request: &str, o: &Opts, stop_after_results: Option<usize>) -> RunInfo {
    let start = Instant::now();
    let mut events: Vec<Value> = Vec::new();
    let mut seen = 0usize;
    let mut bucket_used = false;
    let mut stop_sent = false;
    let mut timed_out = false;
    let mut hard = None::<Instant>;
    app.live_start(request.to_string());
    let drain = |app: &App, events: &mut Vec<Value>, seen: &mut usize, bucket: &mut bool| {
        let log = app.live_log();
        for (kind, text) in log.iter().skip(*seen) {
            if kind == "call" && norm(text).contains("paintbucket") {
                *bucket = true;
            }
            events.push(json!({"brief": n, "t": start.elapsed().as_secs_f64(), "kind": kind, "text": text}));
        }
        *seen = log.len();
    };
    while app.live_active() {
        app.live_pump();
        drain(app, &mut events, &mut seen, &mut bucket_used);
        if let Some(desc) = app.live_pending_confirm() {
            events.push(json!({"brief": n, "t": start.elapsed().as_secs_f64(), "kind": "confirm", "text": desc, "answer": o.auto_confirm}));
            app.live_confirm(o.auto_confirm);
        }
        if let Some(limit) = stop_after_results {
            let results = app.live_log().iter().filter(|(k, _)| k == "ok" || k == "failed").count();
            if !stop_sent && results >= limit {
                events.push(json!({"brief": n, "t": start.elapsed().as_secs_f64(), "kind": "harness", "text": format!("stop after {results} results")}));
                app.live_stop();
                stop_sent = true;
            }
        }
        if !timed_out && start.elapsed() > o.timeout {
            timed_out = true;
            app.live_stop();
            hard = Some(Instant::now() + Duration::from_secs(20));
            events.push(json!({"brief": n, "kind": "harness", "text": "timeout: stop requested"}));
        }
        if hard.is_some_and(|h| Instant::now() > h) {
            app.live_abort("harness timeout");
            events.push(json!({"brief": n, "kind": "harness", "text": "timeout: aborted"}));
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drain(app, &mut events, &mut seen, &mut bucket_used);
    let log_calls = app.live_log().iter().filter(|(k, _)| k == "call").count();
    let (calls, turns) = app.live_last_run().unwrap_or((log_calls, 0));
    let reply = app.live_reply();
    events.push(json!({"brief": n, "kind": "reply", "text": reply}));
    RunInfo { secs: start.elapsed().as_secs_f64(), calls, turns, timed_out, events, bucket_used, reply }
}

fn snapshot(app: &mut App, path: &Path) -> Option<String> {
    let r = agent::execute(app, &AgentCall { tool: "snapshot".into(), args: json!({}) });
    if !r.ok {
        return Some(format!("snapshot failed: {}", r.error.unwrap_or_default()));
    }
    let png = base64::engine::general_purpose::STANDARD.decode(r.value["png_base64"].as_str()?).ok()?;
    std::fs::write(path, png).err().map(|e| e.to_string())
}

struct Row {
    brief: u32,
    pass: bool,
    fails: Vec<String>,
    calls: usize,
    turns: usize,
    secs: f64,
}

/// A headless app, pointed at a local model when `BUZZ_LIVE_LOCAL_MODEL` is
/// set (`BUZZ_LIVE_LOCAL_URL` defaults to LM Studio's port). Without it the
/// saved AI settings decide, as before.
fn new_app() -> App {
    let mut app = App::new(GpuPreference::default());
    if let Ok(model) = std::env::var("BUZZ_LIVE_LOCAL_MODEL") {
        let url = std::env::var("BUZZ_LIVE_LOCAL_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:1234/v1".to_string());
        app.live_use_local(&url, &model);
    }
    app
}

fn main() {
    let o = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}\nusage: agent_live [--brief N] [--out DIR] [--auto-confirm yes|no] [--timeout-secs S]");
            std::process::exit(64);
        }
    };
    let probe = new_app();
    let label = match probe.live_backend() {
        Ok(l) => l,
        Err(guidance) => {
            println!("{guidance}");
            std::process::exit(2);
        }
    };
    drop(probe);
    if let Err(e) = std::fs::create_dir_all(&o.out) {
        eprintln!("cannot create {}: {e}", o.out.display());
        std::process::exit(64);
    }
    let out = std::path::absolute(&o.out).unwrap_or_else(|_| o.out.clone());
    let wav = out.join("agent_live_tone.wav");
    if let Err(e) = write_wav(&wav) {
        eprintln!("cannot write the test WAV: {e}");
        std::process::exit(64);
    }
    println!("backend: {label}");

    // Which briefs run: a brief that builds on others runs them first, in one doc.
    let plan: Vec<u32> = match o.brief {
        None => vec![1, 2, 3, 4, 5, 6],
        Some(2) => vec![1, 2],
        Some(5) => vec![1, 2, 3, 4, 5],
        Some(n) => vec![n],
    };
    let state = |app: &App| app.live_state();
    let mut app = new_app();
    let mut baseline = state(&app);
    let mut rows: Vec<Row> = Vec::new();
    let mut log: Vec<Value> = Vec::new();

    for &n in &plan {
        if n == 6 {
            app = new_app();
        } else if matches!(n, 3 | 4) && o.brief.is_some() {
            app = new_app();
        }
        if n == 1 || n == 6 || (matches!(n, 3 | 4) && o.brief.is_some()) {
            baseline = state(&app);
        }
        let request = match n {
            1 | 6 => BRIEF_1.to_string(),
            2 => BRIEF_2.to_string(),
            3 => BRIEF_3.to_string(),
            4 => brief_4(&wav),
            _ => BRIEF_5.to_string(),
        };
        println!("brief {n}: {request}");
        let before = state(&app);
        let info = run_brief(&mut app, n, &request, &o, if n == 6 { Some(3) } else { None });
        let after = state(&app);
        let snap = snapshot(&mut app, &out.join(format!("brief{n}.png")));
        let mut fails = match n {
            1 => check_brief1(&before, &after),
            2 => check_brief2(&before, &after, info.bucket_used),
            3 => check_brief3(&before, &after),
            4 => check_brief4(&before, &after),
            5 => check_brief5(&baseline, &after),
            _ => {
                let undo = agent::execute(&mut app, &AgentCall { tool: "command".into(), args: json!({"name": "undo"}) });
                let after_undo = state(&app);
                check_brief6(&baseline, &after, &after_undo, false, undo.ok)
            }
        };
        if info.timed_out {
            fails.push("timed out".into());
        }
        if let Some(s) = snap {
            eprintln!("brief {n}: {s}");
        }
        log.push(json!({
            "brief": n, "request": request, "backend": label, "events": info.events,
            "reply": info.reply, "before": before, "after": after, "failures": fails,
            "calls": info.calls, "turns": info.turns, "seconds": info.secs,
        }));
        rows.push(Row { brief: n, pass: fails.is_empty(), fails, calls: info.calls, turns: info.turns, secs: info.secs });
    }

    let log_path = out.join("agent_live_log.json");
    if let Err(e) = std::fs::write(&log_path, serde_json::to_string_pretty(&json!({"backend": label, "briefs": log})).unwrap_or_default()) {
        eprintln!("cannot write {}: {e}", log_path.display());
    }
    println!("\n brief | result | calls | turns | seconds | backend");
    for r in &rows {
        println!(
            " {:>5} | {:<6} | {:>5} | {:>5} | {:>7.1} | {}",
            r.brief,
            if r.pass { "PASS" } else { "FAIL" },
            r.calls,
            r.turns,
            r.secs,
            label
        );
        for f in &r.fails {
            println!("         - {f}");
        }
    }
    println!("log: {}", log_path.display());
    std::process::exit(if rows.iter().all(|r| r.pass) { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(start: i64, tween: &str, objects: Vec<Value>) -> Value {
        json!({"start": start, "tween": tween, "sound": false, "instances": 0, "objects": objects})
    }

    fn oval(fill: &str, x: f64) -> Value {
        json!({"kind": "shape", "oval": true, "fill": fill, "x": x, "y": 100.0})
    }

    fn state(layers: Vec<Value>, depth: i64, label: &str) -> Value {
        json!({"layers": layers, "shapes": 1, "lights": [], "sounds": 0, "undo_depth": depth, "next_undo_label": label})
    }

    fn ball(fill: &str) -> Value {
        json!({"name": "Ball", "keyframes": [
            key(0, "Classic", vec![oval(fill, 50.0)]),
            key(47, "None", vec![oval(fill, 600.0)]),
        ]})
    }

    #[test]
    fn brief1_passes_and_each_criterion_fails_alone() {
        let before = state(vec![], 0, "");
        let good = state(vec![ball("#ff0000")], 1, "AI: x");
        assert!(check_brief1(&before, &good).is_empty(), "{:?}", check_brief1(&before, &good));
        assert!(!check_brief1(&before, &state(vec![ball("#0000ff")], 1, "AI: x")).is_empty());
        assert!(!check_brief1(&before, &state(vec![], 1, "AI: x")).is_empty());
        assert!(!check_brief1(&before, &state(vec![ball("#ff0000")], 3, "AI: x")).is_empty());
        let slow = json!({"name": "Ball", "keyframes": [
            key(0, "Classic", vec![oval("#ff0000", 600.0)]),
            key(47, "None", vec![oval("#ff0000", 50.0)]),
        ]});
        assert!(!check_brief1(&before, &state(vec![slow], 1, "AI: x")).is_empty());
        let no_tween = json!({"name": "Ball", "keyframes": [
            key(1, "None", vec![oval("#ff0000", 50.0)]),
            key(48, "None", vec![oval("#ff0000", 600.0)]),
        ]});
        assert_eq!(check_brief1(&before, &state(vec![no_tween], 1, "AI: x")).len(), 1);
    }

    #[test]
    fn brief2_needs_blue_no_new_shapes_and_the_bucket() {
        let before = state(vec![ball("#ff0000")], 1, "AI: x");
        let after = state(vec![ball("#2050ff")], 2, "AI: y");
        assert!(check_brief2(&before, &after, true).is_empty());
        assert!(!check_brief2(&before, &after, false).is_empty());
        let mut more = after.clone();
        more["shapes"] = json!(2);
        assert!(!check_brief2(&before, &more, true).is_empty());
        assert!(!check_brief2(&before, &state(vec![ball("#ff0000")], 2, "AI: y"), true).is_empty());
    }

    #[test]
    fn brief3_needs_sun_lamp_and_a_brighter_key() {
        let before = state(vec![], 0, "");
        let mut after = state(vec![], 1, "AI: z");
        after["lights"] = json!([
            {"name": "Sun", "kind": "sun", "intensity": 1.0, "track_enabled": false, "keys": []},
            {"name": "Lamp", "kind": "lamp", "intensity": 1.0, "track_enabled": true,
             "keys": [{"frame": 0, "intensity": 0.5}, {"frame": 23, "intensity": 2.0}]},
        ]);
        assert!(check_brief3(&before, &after).is_empty(), "{:?}", check_brief3(&before, &after));
        let mut flat = after.clone();
        flat["lights"][1]["keys"] = json!([{"frame": 23, "intensity": 1.0}]);
        assert!(!check_brief3(&before, &flat).is_empty());
        let mut nosun = after.clone();
        nosun["lights"] = json!([after["lights"][1].clone()]);
        assert!(!check_brief3(&before, &nosun).is_empty());
    }

    #[test]
    fn brief4_needs_sound_attached_and_mouth_keys() {
        let before = state(vec![], 0, "");
        let mut mouth = key(0, "None", vec![]);
        mouth["instances"] = json!(1);
        let mut m2 = mouth.clone();
        m2["start"] = json!(6);
        let mut snd = key(0, "None", vec![]);
        snd["sound"] = json!(true);
        let mut after = state(vec![json!({"name": "Mouth", "keyframes": [mouth, m2]}), json!({"name": "Sound", "keyframes": [snd]})], 1, "AI: w");
        after["sounds"] = json!(1);
        assert!(check_brief4(&before, &after).is_empty(), "{:?}", check_brief4(&before, &after));
        let mut nosound = after.clone();
        nosound["sounds"] = json!(0);
        assert!(!check_brief4(&before, &nosound).is_empty());
    }

    #[test]
    fn briefs5_and_6_compare_documents_not_undo_stacks() {
        let base = state(vec![], 0, "");
        let mut after = state(vec![], 1, "AI: undo");
        assert!(check_brief5(&base, &after).is_empty());
        after["shapes"] = json!(3);
        assert!(!check_brief5(&base, &after).is_empty());

        let changed = state(vec![ball("#ff0000")], 1, "AI: x");
        assert!(check_brief6(&base, &changed, &base, false, true).is_empty());
        assert!(!check_brief6(&base, &changed, &changed, false, true).is_empty());
        assert!(!check_brief6(&base, &changed, &base, true, true).is_empty());
        assert!(!check_brief6(&base, &state(vec![ball("#ff0000")], 3, "AI: x"), &base, false, true).is_empty());
    }
}
