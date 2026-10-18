//! P4 of `docs/PRODUCER_PLAN.md`: a folder of character art → layer rigs in Cast.
//!
//! ```text
//! cargo run --release -p buzz-app --example cast_build -- --dir <chars> --out <dir>
//!     [--names A,B] [--replace] [--assets <lib>] [--wanted <briefs.json>]
//!     [--style comic|painterly|inked] [--launch]
//! ```
//!
//! `--assets` defaults to `scratchpad/produce/p4/assets` (never the user's own
//! library). `--wanted` is a JSON array of `CharacterBrief`s (P1); without it
//! every character found in `--dir` is wanted. `--names` limits either list.
//! ComfyUI draws wanted characters with no picture when it answers (`--launch`
//! starts it when it does not). Writes `<out>/rigs/*.buzz`, `<out>/previews/`
//! and `<out>/cast_report.json`.

use std::path::PathBuf;
use std::time::Duration;

use buzz_ai::breakdown::CharacterBrief;
use buzz_app::backdrop::Style;
use buzz_app::cast_build::{build_cast, scan, CastRequest, GATE_BEYOND_1PX, GATE_MAX_PX};
use buzz_app::comfy::ComfyClient;
use serde_json::json;

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn main() {
    let mut dir: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut names: Option<Vec<String>> = None;
    let mut replace = false;
    let mut assets = PathBuf::from("scratchpad/produce/p4/assets");
    let mut wanted_file: Option<PathBuf> = None;
    let mut style = Style::comic();
    let mut launch = false;
    let mut it = std::env::args().skip(1);
    let usage = "usage: cast_build --dir <chars> --out <dir> [--names A,B] [--replace] [--assets <lib>] [--wanted <json>] [--style comic|painterly|inked] [--launch]";
    while let Some(a) = it.next() {
        let mut next = |what: &str| it.next().unwrap_or_else(|| {
            eprintln!("{a} needs {what}\n{usage}");
            std::process::exit(2)
        });
        match a.as_str() {
            "--dir" => dir = Some(next("a folder").into()),
            "--out" => out = Some(next("a folder").into()),
            "--names" => names = Some(next("names").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()),
            "--replace" => replace = true,
            "--assets" => assets = next("a folder").into(),
            "--wanted" => wanted_file = Some(next("a JSON file").into()),
            "--style" => {
                let s = next("a style");
                style = Style::by_name(&s).unwrap_or(Style { name: "custom".into(), suffix: s });
            }
            "--launch" => launch = true,
            other => {
                eprintln!("unknown argument {other}\n{usage}");
                std::process::exit(2);
            }
        }
    }
    let Some(out) = out else {
        eprintln!("{usage}");
        std::process::exit(2);
    };
    std::fs::create_dir_all(&out).expect("output folder");
    std::fs::create_dir_all(&assets).expect("asset library folder");

    let mut wanted: Vec<CharacterBrief> = match &wanted_file {
        Some(f) => {
            let text = std::fs::read_to_string(f).expect("--wanted file");
            serde_json::from_str(&text).expect("--wanted: a JSON array of character briefs")
        }
        None => dir
            .as_deref()
            .map(scan)
            .unwrap_or_default()
            .into_iter()
            .map(|a| CharacterBrief { name: a.name, importance: 1, ..Default::default() })
            .collect(),
    };
    if let Some(names) = &names {
        let keys: Vec<String> = names.iter().map(|n| norm(n)).collect();
        wanted.retain(|b| std::iter::once(&b.name).chain(b.aliases.iter()).any(|n| keys.contains(&norm(n))));
    }
    if wanted.is_empty() {
        eprintln!("nothing wanted (no characters found, or --names matched none)");
        std::process::exit(2);
    }

    let comfy = if launch {
        match ComfyClient::ensure_running(true, Duration::from_secs(600)) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("ComfyUI: {e}");
                None
            }
        }
    } else {
        ComfyClient::discover()
    };
    println!("ComfyUI: {}", comfy.as_ref().map(|c| c.base_url.as_str()).unwrap_or("none"));

    let library = buzz_doc::AssetLibrary::at(&assets);
    let req = CastRequest { characters_dir: dir, wanted, style, replace, comfy, out: out.clone() };
    let report = build_cast(&library, &req, &mut |line| println!("{line}"));

    let gate = |b: f64, m: f64| if b <= GATE_BEYOND_1PX && m <= GATE_MAX_PX { "pass" } else { "fail" };
    let json = json!({
        "assets": assets.display().to_string(),
        "built": report.built.iter().map(|(s, r, p)| json!({"script": s, "rig": r, "preview": p.display().to_string()})).collect::<Vec<_>>(),
        "generated": report.generated.iter().map(|(s, p)| json!({"script": s, "png": p.display().to_string()})).collect::<Vec<_>>(),
        "skipped": report.skipped.iter().map(|(s, why)| json!({"script": s, "why": why})).collect::<Vec<_>>(),
        "spill": report.spill.iter().map(|(r, b, m)| json!({"rig": r, "beyond_1px": b, "max_px": m, "gate": gate(*b, *m)})).collect::<Vec<_>>(),
        "views": report.views.iter().map(|(r, p, why)| json!({"rig": r, "picture": p.display().to_string(), "why": why})).collect::<Vec<_>>(),
        "notes": report.notes,
    });
    let text = serde_json::to_string_pretty(&json).unwrap_or_default();
    let _ = std::fs::write(out.join("cast_report.json"), &text);
    println!("{text}");
    if report.built.is_empty() {
        std::process::exit(1);
    }
}
