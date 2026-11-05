//! The user's Animate commands as native commands (JSFL_COMMANDS_PLAN J1/J4/J5).
//!
//! These read the user's real Animate folders (read only) and skip themselves
//! when those are not on the machine. The document is `JSFL_TEST_FLA` or the
//! Alaxmi cast file.

use std::path::PathBuf;

use buzz_app::jsfl_commands as j;

fn commands_dir() -> Option<PathBuf> {
    j::animate_commands_dir()
}

#[test]
fn animate_shortcuts_bind_to_the_imported_scripts() {
    let (Some(dir), Some(file)) = (commands_dir(), j::animate_shortcuts_file()) else {
        eprintln!("skipped: no Animate folders on this machine");
        return;
    };
    let scripts = j::scan(&[dir]);
    let keys = j::parse_shortcuts_xml(&std::fs::read_to_string(file).unwrap());
    let r = j::bind_commands(&keys, &scripts, &|_| Vec::new());
    println!("bound {} unmatched {:?}", r.bound.len(), r.unmatched);
    let key = |name: &str, k: egui::Key, ctrl: bool, alt: bool| {
        r.bound.iter().any(|b| {
            b.script == name
                && b.chord.key == k.name()
                && (b.chord.command || b.chord.ctrl) == ctrl
                && b.chord.alt == alt
        })
    };
    // The numpad block moves the eyes; Ctrl+Alt reverses it.
    assert!(key("eyeLeft", egui::Key::Num4, false, false));
    assert!(key("eyeRight", egui::Key::Num6, false, false));
    assert!(key("eyeUp", egui::Key::Num8, false, false));
    assert!(key("RevEyeUp", egui::Key::Num8, true, true));
    assert!(key("switchPoseInteractive", egui::Key::I, false, false));
    assert!(r.unmatched.iter().any(|u| u.contains("rotateEyeBrowUp")), "{:?}", r.unmatched);
}

#[test]
fn the_shelf_scans_and_offers_settings() {
    let Some(dir) = commands_dir() else {
        eprintln!("skipped: no Animate Commands folder");
        return;
    };
    let scripts = j::scan(&[dir]);
    assert!(scripts.len() > 55, "found {}", scripts.len());
    assert!(scripts.iter().all(|s| s.name != "commonVariables"));
    assert!(scripts.iter().filter(|s| s.internal).count() >= 3);

    let with: Vec<usize> = scripts.iter().map(|s| j::tunables_of(s).0.len()).filter(|n| *n > 0).collect();
    println!(
        "scripts with settings: {}, values: {} (+ {} shared)",
        with.len(),
        with.iter().sum::<usize>(),
        j::shared_tunables(&scripts[0].config_dir).len()
    );
    let find = |n: &str| scripts.iter().find(|s| s.name == n).unwrap();
    // Three real scripts' tunables.
    let (own, shared) = j::tunables_of(find("characterSetup"));
    let names: Vec<_> = own.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"SHADOW_OFFSET_X"), "{names:?}");
    assert!(names.contains(&"BREATHING_SQUASH"), "{names:?}");
    assert!(shared.is_empty());
    let (own, _) = j::tunables_of(find("Animate_HandBodyEyeMovement"));
    assert!(own.iter().any(|t| t.name == "delayFrames" && t.default == "25"), "{own:?}");
    let (_, shared) = j::tunables_of(find("eyeLeft"));
    let shared_names: Vec<_> = shared.iter().map(|t| t.name.as_str()).collect();
    assert!(shared_names.contains(&"CONFIG.moveAmount"), "{shared_names:?}");
    assert!(shared_names.contains(&"moveFrames"), "{shared_names:?}");
}

/// Enter the first character, then its `head` symbol, so the eye layers are
/// the timeline the script sees.
const ENTER_HEAD: &str = r#"
var __d = fl.getDocumentDOM();
function __enter(name) {
  var t = __d.getTimeline();
  for (var i = 0; i < t.layers.length; i++) {
    if (name && t.layers[i].name !== name) continue;
    var f = t.layers[i].frames[0];
    if (f && f.elements.length && f.elements[0].elementType == 'instance') {
      __d.selection = [f.elements[0]]; __d.enterEditMode('inPlace'); __d.selectNone(); return;
    }
  }
}
__enter(''); __enter(''); __enter('head');
__d.getTimeline().currentFrame = 10;
"#;

/// The x of the eyeBall at frame `at`, read by a script after the command ran.
fn eye_x(settings: &j::JsflStore, fla: &std::path::Path) -> (f64, f64) {
    let dir = commands_dir().unwrap();
    let scripts = j::scan(&[dir]);
    let entry = scripts.iter().find(|s| s.name == "eyeLeft").unwrap();
    let (command, shared) = settings.prepare(entry).unwrap();

    let doc = buzz_app::import::read(fla).unwrap().document();
    let mut film = buzz_script::Film {
        scenes: doc.film(),
        names: doc.scene_names(),
        current: 0,
        sounds: Vec::new(),
    };
    let source = format!(
        "{ENTER_HEAD}\n{command}\n;(function(){{ var L = __d.getTimeline().layers; \
         for (var i = 0; i < L.length; i++) if (L[i].name == 'eyeBall') {{ \
         fl.trace('X ' + L[i].frames[10].elements[0].x + ' ' + L[i].frames[10 + CONFIG.moveFrames].elements[0].x); }} }})();"
    );
    let context = buzz_script::ScriptContext {
        config_dir: entry.config_dir.clone().into(),
        overrides: shared,
        ..Default::default()
    };
    let out = buzz_script::run_film(&mut film, context, &source, &buzz_script::Limits::default(), None);
    assert!(out.error.is_none(), "{:?}", out.error);
    let line = out.trace.iter().find(|t| t.starts_with("X ")).expect("the eye layer was read");
    let mut it = line[2..].split_whitespace().map(|v| v.parse::<f64>().unwrap());
    (it.next().unwrap(), it.next().unwrap())
}

#[test]
fn a_changed_setting_moves_the_eye_by_the_new_amount() {
    let fla = PathBuf::from(
        std::env::var("JSFL_TEST_FLA")
            .unwrap_or_else(|_| r"B:\youtubeProjects\Khayal3Baje\stories\alaxmi\BuzzAnimate\cast.fla".into()),
    );
    if commands_dir().is_none() || !fla.exists() {
        eprintln!("skipped: no Animate folder or document");
        return;
    }
    let mut store = j::JsflStore::default();
    let (start, end) = eye_x(&store, &fla);
    // eyeLeft moves the eye left by CONFIG.moveAmount (5 in commonVariables).
    assert!((start - end - 5.0).abs() < 1e-6, "{start} -> {end}");

    // The shared value, changed once, applies to the command that includes it.
    store.set(j::SHARED_STEM, "CONFIG.moveAmount", Some("11".into()));
    let (start, end) = eye_x(&store, &fla);
    assert!((start - end - 11.0).abs() < 1e-6, "{start} -> {end}");

    // Reset puts the script's own value back.
    store.set(j::SHARED_STEM, "CONFIG.moveAmount", None);
    let (start, end) = eye_x(&store, &fla);
    assert!((start - end - 5.0).abs() < 1e-6, "{start} -> {end}");
}
