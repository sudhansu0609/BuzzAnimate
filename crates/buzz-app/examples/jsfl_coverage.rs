//! **JSFL coverage harness** (JSFL_COMMANDS_PLAN J1).
//!
//! `jsfl_coverage --doc <cast.fla|x.buzz> [--commands <dir>] [--only <substr>] [--frame N] [--setup "<js>"] [--pick <picture>]`
//!
//! Runs every `.jsfl` of the Commands folder against a copy of the document
//! (each from a fresh copy, nothing is saved), with a per-script-family setup
//! line prepended (playhead, selection), and prints one line per script:
//! `ran`/`threw`, whether the document changed, the error, and the
//! missing-API name when the error is `x is not a function` / `x is not defined`.
//! The commands folder and the document are only read.

use std::path::PathBuf;

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

/// Which timeline a script's author would have open when running it.
fn level_for(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("pose") || n.contains("charactersetup") {
        "charsel"
    } else if n.contains("handbody") {
        "body"
    } else if n.contains("eye") || n.contains("brow") || n.contains("mouth") || n.contains("headlayers") {
        "head"
    } else if n.contains("hand")
        || n.contains("wrist")
        || n.contains("palm")
        || n.contains("talking")
        || n.contains("lipsync")
        || n.contains("layerparenting")
        || n.contains("renamelayers")
        || n.contains("shadow")
        || n.contains("extendframes")
        || n.contains("setframeofsymbols")
    {
        "body"
    } else {
        "root"
    }
}

/// JS prepended to a script: the timeline entered (root, the character's body,
/// its head), the playhead and the selection, the way the script's author would
/// have left Animate before running it. Also defines `__snap(tag)`, which traces
/// one `§tag|key|value` line per keyframe in a window round the playhead so the
/// harness can print what the script changed.
fn setup_for(name: &str, frame: u32, level: &str) -> String {
    let mut js = String::from(
        r#"
var __d = fl.getDocumentDOM();
function __enterFirst(layerName) {
  var t = __d.getTimeline();
  for (var i = 0; i < t.layers.length; i++) {
    if (layerName && t.layers[i].name !== layerName) continue;
    var f = t.layers[i].frames[0];
    if (f && f.elements && f.elements.length && f.elements[0].elementType == 'instance') {
      __d.selection = [f.elements[0]]; __d.enterEditMode('inPlace'); __d.selectNone(); return true;
    }
  }
  return false;
}
function __snap(tag) {
  var t = __d.getTimeline(); var cf = t.currentFrame;
  var lo = Math.max(0, cf - 2), hi = cf + 40;
  fl.trace('§' + tag + '|timeline|layers=' + t.layers.length + ' frames=' + t.frameCount);
  for (var i = 0; i < t.layers.length; i++) {
    var L = t.layers[i]; var n = L.frameCount;
    fl.trace('§' + tag + '|L' + i + ' ' + L.name + '|type=' + L.layerType + ' vis=' + L.visible + ' lock=' + L.locked + ' frames=' + n);
    for (var f = lo; f < hi && f < n; f++) {
      var fr = L.frames[f];
      if (!fr || !fr.isKeyframe) continue;
      var els = fr.elements; var s = 'tween=' + fr.tweenType + ' n=' + els.length;
      if (els.length) { var e = els[0]; s += ' x=' + Math.round(e.x * 10) / 10 + ' y=' + Math.round(e.y * 10) / 10;
        if (e.rotation !== undefined) s += ' rot=' + Math.round(e.rotation * 10) / 10;
        if (e.firstFrame !== undefined) s += ' ff=' + e.firstFrame; }
      fl.trace('§' + tag + '|L' + i + ' ' + L.name + ' f' + f + '|' + s);
    }
  }
}
"#,
    );
    match level {
        "body" => js.push_str("__enterFirst(''); __enterFirst('');
"),
        "charsel" => js.push_str(
            "__enterFirst(''); __d.getTimeline().currentFrame = 0; __d.selection = [__d.getTimeline().layers[0].frames[0].elements[0]];
",
        ),
        "head" => js.push_str("__enterFirst(''); __enterFirst(''); __enterFirst('head');
"),
        _ => {}
    }
    // The character's own timeline is one frame long.
    let frame = if level == "charsel" { 0 } else { frame };
    js.push_str(&format!("__d.getTimeline().currentFrame = {frame};
"));
    let n = name.to_ascii_lowercase();
    let wants_selection = n.contains("renamesymbol")
        || n.contains("setframeofsymbols")
        || n.contains("shadow");
    if wants_selection {
        js.push_str("__d.selectAll();
");
    }
    js.push_str("__snap('before');
");
    js
}

/// Print what changed between the `before` and `after` snapshots.
fn print_diff(trace: &[String]) {
    use std::collections::BTreeMap;
    let mut before = BTreeMap::new();
    let mut after = BTreeMap::new();
    for line in trace {
        if let Some(rest) = line.strip_prefix('§') {
            let mut it = rest.splitn(3, '|');
            let (tag, key, val) = (it.next().unwrap(), it.next().unwrap_or(""), it.next().unwrap_or(""));
            let map = if tag == "before" { &mut before } else { &mut after };
            map.insert(key.to_string(), val.to_string());
        }
    }
    if after.is_empty() {
        println!("   (no after-snapshot)");
        return;
    }
    let mut n = 0;
    for (k, v) in &after {
        if before.get(k) != Some(v) {
            n += 1;
            if n <= 40 {
                match before.get(k) {
                    Some(b) => println!("   ~ {k}: {b}  ->  {v}"),
                    None => println!("   + {k}: {v}"),
                }
            }
        }
    }
    for (k, v) in &before {
        if !after.contains_key(k) {
            n += 1;
            if n <= 40 {
                println!("   - {k}: {v}");
            }
        }
    }
    println!("   diff lines: {n}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let doc_path = PathBuf::from(arg(&args, "--doc").expect("--doc <file>"));
    let dir = PathBuf::from(arg(&args, "--commands").unwrap_or_else(|| {
        let mut p = buzz_script::default_config_dir().expect("no config dir");
        p.push("Commands");
        p.to_string_lossy().to_string()
    }));
    let only = arg(&args, "--only");
    let frame: u32 = arg(&args, "--frame").and_then(|f| f.parse().ok()).unwrap_or(10);
    let extra_setup = arg(&args, "--setup").unwrap_or_default();
    let verbose = args.iter().any(|a| a == "--verbose");
    let pick = arg(&args, "--pick").map(PathBuf::from);

    let doc = if doc_path.extension().is_some_and(|e| e == "buzz") {
        buzz_doc::Document::open(&doc_path).expect("open buzz")
    } else {
        buzz_app::import::read(&doc_path).expect("import").document()
    };
    eprintln!("document: {} scene(s)", doc.scene_names().len());

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("commands dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsfl")))
        .collect();
    files.sort();

    let mut ok = 0;
    let mut changed_n = 0;
    let mut threw = 0;
    for path in &files {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        if let Some(o) = &only
            && !name.contains(o.as_str())
        {
            continue;
        }
        let body = match std::fs::read(path) {
            Ok(b) => String::from_utf8_lossy(&b).to_string(),
            Err(e) => {
                println!("READFAIL\t{name}\t{e}");
                continue;
            }
        };
        let level = arg(&args, "--level").unwrap_or_else(|| level_for(&name).to_string());
        let footer = "
;__snap('after');
";
        let source = format!("{}{}
{}{}", setup_for(&name, frame, &level), extra_setup, body, footer);
        let mut film = buzz_script::Film {
            scenes: doc.film(),
            names: doc.scene_names(),
            current: 0,
            sounds: Vec::new(),
        };
        let context = buzz_script::ScriptContext {
            current_frame: 0,
            selection: Vec::new(),
            active_layer: None,
            config_dir: dir.parent().map(|p| p.to_path_buf()),
            asset_root: None,
            prompt_answers: Vec::new(),
            // The hooks the app gives a run (Trace Bitmap) and, with `--pick`, a
            // dialog that answers by itself: the picked file, each prompt's own
            // default, yes, and every XMLUI control as it starts.
            hooks: buzz_app::jsfl_hooks::hooks(pick.clone().map(|file| -> buzz_script::AskFn {
                std::sync::Arc::new(move |q| match q {
                    buzz_script::Question::PickFile { .. } => buzz_script::Answer::File(file.clone()),
                    buzz_script::Question::Prompt { default, .. } => buzz_script::Answer::Text(default),
                    buzz_script::Question::Confirm { .. } => buzz_script::Answer::Yes(true),
                    buzz_script::Question::Form(f) => buzz_script::Answer::Form(f.defaults()),
                })
            })),
            overrides: Vec::new(),
        };
        let limits = buzz_script::Limits {
            time: std::time::Duration::from_secs(60),
            memory: 512 * 1024 * 1024,
            ..buzz_script::Limits::default()
        };
        let out = buzz_script::run_film(&mut film, context, &source, &limits, None);
        match &out.error {
            None => {
                ok += 1;
                if out.changed {
                    changed_n += 1;
                }
                println!(
                    "RAN\t{name}\tchanged={}\t{}ms\ttrace={}\talerts={}",
                    out.changed,
                    out.elapsed.as_millis(),
                    out.trace.iter().filter(|t| !t.starts_with('§')).count(),
                    out.alerts.len()
                );
            }
            Some(e) => {
                threw += 1;
                println!("THREW\t{name}\tchanged={}\t{}", out.changed, e.replace("\n", " | "));
                // The failing line of the script itself (the setup above it
                // is `offset_lines` lines long).
                if let Some(at) = e.find("eval_script:") {
                    let num: String = e[at + 12..].chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = num.parse::<usize>() {
                        let offset = source.len() - body.len() - footer.len();
                        let offset_lines = source[..offset].matches('\n').count();
                        if n > offset_lines {
                            let line = body.lines().nth(n - offset_lines - 1).unwrap_or("");
                            println!("   at script line {}: {}", n - offset_lines, line.trim());
                        }
                    }
                }
            }
        }
        if verbose {
            for t in out.trace.iter().filter(|t| !t.starts_with('§')).take(300) {
                println!("   trace: {t}");
            }
            print_diff(&out.trace);
            for a in out.alerts.iter().take(300) {
                println!("   alert: {a}");
            }
        }
    }
    println!("TOTAL ran={ok} (changed {changed_n}) threw={threw}");
}
