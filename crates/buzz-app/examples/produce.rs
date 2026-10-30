//! **The producer from the command line** (PRODUCER_PLAN § P6): a script, a
//! recording and a folder of character art in, a finished episode out.
//!
//! ```text
//! cargo run --release -p buzz-app --example produce -- \
//!     --script ep1/script.txt --out out/ [--audio full_episode.wav] [--audio-parts <folder of ~3-min parts>] [--part N] [--characters ep1/] \
//!     [--style comic|painterly|inked|"free text"] [--series Name] [--world "rural Bihar, India, late 1990s"] \
//!     [--scenes 4-6] [--assets <library>] [--index <.buzz-index.json>] [--size 1920x1080] \
//!     [--no-backgrounds] [--mp4] [--height 540] [--stills 4] [--launch-comfy] [--offline] [--fresh] [--objects|--no-objects] [--captions on|off] [--story-dir <story folder>] [--allow-generated-cast] [--redo <step>] [--cancel-after <s>]
//! ```
//!
//! Everything lands in `--out`: `breakdown.json`, `cast.json`,
//! `backgrounds.json` (+ `backgrounds/*.png`), `audio.json`, `casting.json`,
//! `<series>.buzz`, `scenes/*.mp4` + `<series>.mp4` with `--mp4`, `stills/`
//! with `--stills N`, `report.md` (+ `parts.json`, `audio/full.wav`, `audio/parts/` from `--audio-parts` / `--audio`; `--part N` writes `part_N.mp4`, `part_N/`), and `produce.json`, which lets a second run
//! skip every step whose inputs did not change (`--fresh` redoes them all;
//! `--redo backgrounds` redoes that step and every later one).
//! `--assets` is the asset library rigs and backgrounds are saved into
//! (default: the user's).

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use buzz_app::backdrop::Style;
use buzz_app::produce::{ProduceRequest, Step, produce};

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
    let mut script = None;
    let mut out = None;
    let mut req_style = Style::comic();
    let mut audio = None;
    let mut audio_parts = None;
    let mut part: Option<usize> = None;
    let mut characters = None;
    let mut series = String::new();
    let mut world = String::new();
    let mut scenes = None;
    let mut assets = None;
    let mut index = None;
    let mut size = (1920.0, 1080.0);
    let mut backgrounds = true;
    let mut mp4 = false;
    let mut height = 540u32;
    let mut stills = 0u32;
    let mut launch = false;
    let mut offline = false;
    let mut fresh = false;
    let mut objects: Option<bool> = None;
    let mut captions: Option<bool> = None;
    let mut story_dir: Option<PathBuf> = None;
    let mut allow_generated_cast = false;
    let mut redo = None;
    let mut cancel_after: Option<f64> = None;
    while let Some(flag) = args.next() {
        let mut value = |what: &str| -> Result<String> { args.next().with_context(|| format!("{what} needs a value")) };
        match flag.as_str() {
            "--script" => script = Some(PathBuf::from(value("--script")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--audio" => audio = Some(PathBuf::from(value("--audio")?)),
            "--audio-parts" => audio_parts = Some(PathBuf::from(value("--audio-parts")?)),
            "--part" => part = Some(value("--part")?.parse()?),
            "--characters" => characters = Some(PathBuf::from(value("--characters")?)),
            "--style" => {
                let text = value("--style")?;
                req_style = Style::by_name(&text).unwrap_or(Style { name: "custom".into(), suffix: text });
            }
            "--series" => series = value("--series")?,
            "--world" => world = value("--world")?,
            "--scenes" => scenes = Some(parse_scenes(&value("--scenes")?)?),
            "--assets" => assets = Some(PathBuf::from(value("--assets")?)),
            "--index" => index = Some(PathBuf::from(value("--index")?)),
            "--size" => {
                let text = value("--size")?;
                let (w, h) = text.split_once(['x', 'X']).context("--size is WIDTHxHEIGHT, like 1920x1080")?;
                size = (w.trim().parse()?, h.trim().parse()?);
            }
            "--no-backgrounds" => backgrounds = false,
            "--mp4" => mp4 = true,
            "--height" => height = value("--height")?.parse()?,
            "--stills" => stills = value("--stills")?.parse()?,
            "--launch-comfy" => launch = true,
            "--offline" => offline = true,
            "--fresh" => fresh = true,
            "--objects" => objects = Some(true),
            "--no-objects" => objects = Some(false),
            "--captions" => {
                let v = value("--captions")?;
                captions = Some(match v.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => bail!("--captions is on or off"),
                });
            }
            "--story-dir" => story_dir = Some(PathBuf::from(value("--story-dir")?)),
            "--allow-generated-cast" => allow_generated_cast = true,
            "--cancel-after" => cancel_after = Some(value("--cancel-after")?.parse()?),
            "--redo" => {
                let v = value("--redo")?;
                redo = Some(Step::from_key(&v).with_context(|| format!("--redo {v}: parse, breakdown, cast, backgrounds, audio, build, render or report"))?);
            }
            other => bail!("unknown argument {other:?}"),
        }
    }
    let script = script.context("--script <script.txt> is required")?;
    let out = out.context("--out <dir> is required")?;
    let mut req = ProduceRequest::new(script, out);
    req.audio = audio;
    req.audio_parts = audio_parts;
    req.part = part;
    req.characters_dir = characters;
    req.style = req_style;
    req.series = series;
    req.world = world;
    req.scenes = scenes;
    req.assets = assets;
    req.index = index;
    req.size = size;
    req.backgrounds = backgrounds;
    req.mp4 = mp4;
    req.height = height;
    req.stills = stills;
    req.launch_comfy = launch;
    req.offline = offline;
    req.fresh = fresh;
    req.objects = objects;
    req.captions = captions;
    req.story_dir = story_dir;
    req.allow_generated_cast = allow_generated_cast;
    req.redo = redo;

    let started = Instant::now();
    let mut progress = |step: Step, line: &str| {
        println!("[{:>7.1}s] {}: {line}", started.elapsed().as_secs_f64(), step.label());
    };
    // `--cancel-after S`: press Cancel S seconds in (P8-C2 check); the time
    // from then until `produce` returns is printed.
    let cancel = || cancel_after.is_some_and(|s| started.elapsed().as_secs_f64() >= s);
    let result = produce(&req, &mut progress, &cancel);
    if let Some(s) = cancel_after {
        let now = started.elapsed().as_secs_f64();
        println!("cancel at {s:.1} s; produce returned at {now:.2} s ({:.2} s after the cancel): {}", now - s, match &result {
            Ok(_) => "finished".to_string(),
            Err(e) => format!("{e}"),
        });
    }
    let report = result?;
    println!("\nDone in {:.1} s", started.elapsed().as_secs_f64());
    for (step, secs) in &report.seconds {
        println!("  {step:<12} {secs:>8.1} s");
    }
    println!("skipped (unchanged): {:?}", report.skipped_steps);
    println!("breakdown: {}", report.breakdown_backend);
    println!("cast: {:?}", report.cast);
    println!("generated characters: {:?}", report.generated_characters);
    println!("backgrounds: {:?}", report.backgrounds);
    println!("lines from audio {}, from text {}; cuts {}", report.lines_from_audio, report.lines_from_text, report.cuts);
    println!("buzz {}", report.buzz.display());
    if let Some(m) = &report.mp4 {
        println!("mp4 {}", m.display());
    }
    for s in &report.stills {
        println!("still {}", s.display());
    }
    println!("report {}", report.report.display());
    for w in &report.warnings {
        println!("warning: {w}");
    }
    Ok(())
}
