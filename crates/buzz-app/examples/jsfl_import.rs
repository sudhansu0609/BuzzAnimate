//! `jsfl_import [--write]`: the Import Animate Commands / Import Animate
//! Shortcuts buttons, run headlessly (JSFL_COMMANDS_PLAN J1, J4).
//!
//! Prints what it would do as JSON. With `--write` it copies the scripts into
//! the app's Commands folder and stores the key bindings in the app's
//! `jsfl.json`; BuzzAnimate unbinds the displaced built-in shortcuts when it
//! next starts. Animate's own files are only read.

use buzz_app::jsfl_commands as j;
use buzz_ui::command::{Command, all_with_shortcuts};
use buzz_ui::workspace::{KeyChord, command_id};

fn main() {
    let write = std::env::args().any(|a| a == "--write");
    let mut store = j::JsflStore::load(&j::store_path());
    let from = store.commands_dir.clone().or_else(j::animate_commands_dir).expect("no Animate Commands folder");

    let mut report = serde_json::Map::new();
    if write {
        let r = j::import_animate_commands(&from, &j::app_commands_dir()).expect("import");
        report.insert("copied".into(), r.copied.len().into());
        report.insert("skipped".into(), r.skipped.iter().map(|(n, why)| format!("{n}: {why}")).collect::<Vec<_>>().into());
    }
    let scripts = j::scan(&[j::app_commands_dir(), from]);
    report.insert("scripts".into(), scripts.len().into());

    let file = j::animate_shortcuts_file().expect("no Animate shortcut file");
    report.insert("shortcut_file".into(), file.display().to_string().into());
    let keys = j::parse_shortcuts_xml(&std::fs::read_to_string(&file).expect("read"));

    // BuzzAnimate's shortcuts as they ship (a saved layout's overrides are the
    // app's to apply at startup).
    let all: Vec<(Command, String, KeyChord)> = all_with_shortcuts()
        .into_iter()
        .filter_map(|c| c.shortcut().map(|s| (c, command_id(c), KeyChord::from_shortcut(s))))
        .collect();
    let r = j::bind_commands(&keys, &scripts, &|chord| {
        all.iter().filter(|(_, _, b)| j::same_chord(b, chord)).map(|(_, id, _)| id.clone()).collect()
    });
    report.insert("bound".into(), r.bound.len().into());
    report.insert(
        "conflicts".into(),
        r.conflicts
            .iter()
            .map(|c| format!("{} on {} displaces {}", c.script, c.chord.label(), c.displaced))
            .collect::<Vec<_>>()
            .into(),
    );
    report.insert("unmatched".into(), r.unmatched.clone().into());
    let unbound_scripts: Vec<String> = scripts
        .iter()
        .filter(|s| !s.internal && !r.bound.iter().any(|b| b.script == s.name))
        .map(|s| s.name.clone())
        .collect();
    report.insert("scripts_without_a_key".into(), unbound_scripts.len().into());

    let default_for = |buzz: &str| -> Option<KeyChord> {
        all.iter().find(|(_, id, _)| id == buzz).map(|(_, _, c)| c.clone())
    };
    let (diffs, none) = j::other_differences(&keys, &default_for);
    report.insert("other_differences".into(), diffs.len().into());
    report.insert("other_difference_lines".into(), diffs.into());
    report.insert("other_without_buzz_counterpart".into(), none.into());

    if write {
        store.keys = r.bound.clone();
        store.save(&j::store_path()).expect("save jsfl.json");
        report.insert("store".into(), j::store_path().display().to_string().into());
    }
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
