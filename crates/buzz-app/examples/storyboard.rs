//! **A storyboard for an episode** (`docs/STORYBOARD_PLAN.md` S1–S3).
//!
//! ```text
//! cargo run --release -p buzz-app --example storyboard -- <episode dir> --scenes 4,5 --out <dir> \
//!     [--look auto|a|b] [--offline] [--breakdown <breakdown.json>] [--assets <library>] //!     [--world "rural Bihar, India, late 1990s"]
//! ```
//!
//! `--breakdown` takes the breakdown of an earlier run as it is: no model is asked.
//!
//! `<episode dir>` holds `script.txt` and `buzz/casting.json` (or `casting.json`).
//! Everything lands in `--out`: `breakdown.json` (kept for the next run),
//! `storyboard.json` (the board; edits made to it survive a re-run),
//! `panels/<id>.png`, `sketch/` (look B), `board_p01.png`… and `board.pdf`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use buzz_app::storyboard::{LookChoice, PANEL_SIZE, StoryboardRequest, make};

fn parse_scenes(text: &str) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            let (a, b): (u32, u32) = (a.trim().parse()?, b.trim().parse()?);
            out.extend(a..=b);
        } else {
            out.push(part.parse()?);
        }
    }
    Ok(out)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<PathBuf> = None;
    let mut scenes = None;
    let mut out = None;
    let mut look = LookChoice::Auto;
    let mut offline = false;
    let mut assets = None;
    let mut world = String::new();
    let mut breakdown = None;
    while let Some(flag) = args.next() {
        let mut value = |what: &str| -> Result<String> { args.next().with_context(|| format!("{what} needs a value")) };
        match flag.as_str() {
            "--scenes" => scenes = Some(parse_scenes(&value("--scenes")?)?),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--look" => {
                look = match value("--look")?.to_lowercase().as_str() {
                    "auto" => LookChoice::Auto,
                    "a" => LookChoice::A,
                    "b" => LookChoice::B,
                    other => bail!("--look {other}: auto, a or b"),
                }
            }
            "--offline" => offline = true,
            "--assets" => assets = Some(PathBuf::from(value("--assets")?)),
            "--world" => world = value("--world")?,
            "--breakdown" => breakdown = Some(PathBuf::from(value("--breakdown")?)),
            other if !other.starts_with("--") && dir.is_none() => dir = Some(PathBuf::from(other)),
            other => bail!("unknown argument {other:?}"),
        }
    }
    let dir = dir.context("the episode folder (with script.txt) is required")?;
    let out = out.context("--out <dir> is required")?;
    let casting = [dir.join("buzz").join("casting.json"), dir.join("casting.json")]
        .into_iter()
        .find(|p| p.is_file())
        .with_context(|| format!("no casting.json in {} or its buzz folder", dir.display()))?;
    let req = StoryboardRequest {
        script: dir.join("script.txt"),
        casting,
        out,
        scenes,
        look,
        offline,
        assets,
        world,
        size: PANEL_SIZE,
        breakdown,
    };
    let started = std::time::Instant::now();
    let report = make(&req, &mut |line| println!("[{:>6.1}s] {line}", started.elapsed().as_secs_f64()))?;
    println!("\nboard {}", report.board.display());
    println!("breakdown: {}", report.breakdown_backend);
    for (scene, n) in &report.panels {
        println!("scene {scene}: {n} panel(s)");
    }
    for p in &report.pages {
        println!("page {}", p.display());
    }
    match &report.pdf {
        Some(p) => println!("pdf {}", p.display()),
        None => println!("pdf: none"),
    }
    for f in &report.framing {
        println!("framing: {f}");
    }
    for l in &report.look_b {
        println!("look B: {l}");
    }
    for w in &report.warnings {
        println!("warning: {w}");
    }
    Ok(())
}
