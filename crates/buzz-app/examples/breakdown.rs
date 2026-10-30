//! **A script in, its breakdown out** (Producer P1): characters, places,
//! settings, background prompts, who stands where, props, lights and shots.
//!
//! ```text
//! cargo run --release -p buzz-app --example breakdown -- \
//!     --script ep1/script.txt --out breakdown.json [--offline] \
//!     [--backend claude|lmstudio] [--model <id>] \
//!     [--style "comic, light humour"] [--series Chirailiya] [--known Guddu,Munna] \
//!     [--world "rural Bihar, India, late 1990s"] [--from breakdown.json] \
//!     [--cast GUDDU=Guddu,AMMA=Woman1 --casting-out casting.json]
//! ```
//!
//! `--from` reads a breakdown written before instead of making one: it is
//! tidied again (`buzz_ai::breakdown::tidy`), written to `--out`, and turned
//! into a casting file, with no model call.
//!
//! `--offline` uses no model: the script's own rules only
//! (`buzz_ai::breakdown::breakdown_offline`). Otherwise the backend is the
//! Claude API server when AI ▸ Models has Claude switched on (`use_claude`),
//! else LM Studio at `127.0.0.1:1234` with a loaded Gemma, any Gemma, or its
//! first chat model; `--backend`
//! and `--model` choose instead. A model that fails or answers badly costs only
//! its batch: those scenes keep the offline answer, and the run says so.
//!
//! `--cast` maps script names to Cast rigs; with `--casting-out` the breakdown
//! is also written as a casting file (`buzz_ai::breakdown::casting_value`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use buzz_act::screenplay::Screenplay;
use buzz_ai::breakdown::{self, BreakdownHints};
use buzz_ai::llm::Backend;

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn list(value: Option<String>) -> Vec<String> {
    value
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

fn write_json(path: &PathBuf, value: &impl serde::Serialize) -> Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {dir:?}"))?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?).with_context(|| format!("writing {path:?}"))
}

/// LM Studio's model for a breakdown: a loaded Gemma (they answer this fast
/// and held up on Ep1, where the first-listed qwen 27B died), else any Gemma,
/// else the first chat model. Embedding models cannot answer.
fn lmstudio_model(backend: &Backend) -> (String, &'static str) {
    let chat: Vec<String> = backend
        .list_models()
        .unwrap_or_default()
        .into_iter()
        .filter(|id| !id.to_lowercase().contains("embed"))
        .collect();
    // LM Studio's own API says which models are loaded; `/v1/models` does not.
    let loaded: Vec<String> = ureq::get(&format!("{}/api/v0/models", backend.base_url().trim_end_matches("/v1")))
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .ok()
        .and_then(|r| r.into_json::<serde_json::Value>().ok())
        .and_then(|v| {
            v["data"].as_array().map(|list| {
                list.iter()
                    .filter(|m| m["state"].as_str() == Some("loaded"))
                    .filter_map(|m| m["id"].as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default();
    let gemma = |id: &&String| id.to_lowercase().contains("gemma");
    if let Some(m) = loaded.iter().filter(|id| chat.contains(id)).find(gemma) {
        (m.clone(), "a loaded Gemma")
    } else if let Some(m) = chat.iter().find(gemma) {
        (m.clone(), "no Gemma loaded; the first Gemma listed")
    } else {
        (chat.first().cloned().unwrap_or_default(), "no Gemma; the first chat model")
    }
}

fn main() -> Result<()> {
    let Some(script) = arg_value("--script") else {
        bail!("--script <file> is required");
    };
    let Some(out) = arg_value("--out").map(PathBuf::from) else {
        bail!("--out <json> is required");
    };
    let bytes = std::fs::read(&script).with_context(|| format!("reading {script}"))?;
    let play = Screenplay::parse(&String::from_utf8_lossy(&bytes));
    if play.scenes.is_empty() {
        bail!("{script}: no scenes found");
    }
    let hints = BreakdownHints {
        style: arg_value("--style").unwrap_or_else(|| "comic, light humour".into()),
        known_characters: list(arg_value("--known")),
        series: arg_value("--series").unwrap_or_else(|| play.title.clone()),
        world: arg_value("--world").unwrap_or_default(),
        ..Default::default()
    };
    println!("{}: {} scenes", play.title, play.scenes.len());

    let started = Instant::now();
    let result = if let Some(from) = arg_value("--from") {
        // A breakdown made before: tidied again and written out, no model.
        let text = std::fs::read_to_string(&from).with_context(|| format!("reading {from}"))?;
        let mut b: breakdown::Breakdown = serde_json::from_str(&text).with_context(|| format!("parsing {from}"))?;
        if !hints.world.is_empty() {
            b.world = hints.world.clone();
        }
        breakdown::tidy(&mut b, &play);
        println!("from {from}");
        b
    } else if std::env::args().any(|a| a == "--offline") {
        println!("offline");
        breakdown::breakdown_offline(&play, &hints)
    } else {
        let which = arg_value("--backend").unwrap_or_else(|| {
            if buzz_ai::ai_config::use_claude() { "claude".into() } else { "lmstudio".into() }
        });
        let backend = match which.as_str() {
            "claude" => Backend::default_claude_api(),
            "lmstudio" => Backend::default_openai(),
            other => bail!("--backend {other}: claude or lmstudio"),
        };
        let (model, why) = match arg_value("--model") {
            Some(m) => (m, "--model"),
            None if which == "claude" => (buzz_ai::ai_config::claude_model(), "the saved Claude model"),
            None => lmstudio_model(&backend),
        };
        // Breakdown is reading, not reasoning: ask reasoning models for little.
        buzz_ai::llm::set_low_reasoning(true);
        println!("backend {which} ({}), model {model:?} ({why})", backend.display_name());
        let mut progress = |line: &str| println!("[{:>6.1}s] {line}", started.elapsed().as_secs_f64());
        progress(&format!("model {model} ({why})"));
        breakdown::breakdown(&backend, &model, &play, &hints, &mut progress)
    };
    let seconds = started.elapsed().as_secs_f64();

    write_json(&out, &result)?;
    println!(
        "{} characters, {} scenes, {} places in {seconds:.1} s -> {}",
        result.characters.len(),
        result.scenes.len(),
        result
            .scenes
            .iter()
            .map(|s| s.location_key.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        out.display()
    );

    if let Some(casting_out) = arg_value("--casting-out").map(PathBuf::from) {
        let cast: BTreeMap<String, String> = list(arg_value("--cast"))
            .into_iter()
            .filter_map(|pair| pair.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();
        write_json(&casting_out, &breakdown::casting_value(&result, &cast))?;
        println!("casting -> {}", casting_out.display());
    }
    Ok(())
}
