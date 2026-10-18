//! **A screenplay and a casting file in, an episode out.**
//!
//! ```text
//! cargo run --release -p buzz-app --example episode_build -- \
//!     --script ep1/script.txt --casting ep1/casting.json \
//!     --scenes 4,5,6 --out out/ [--mp4] [--stills 4] [--frames 400,600] [--height 540] //!     [--assets <dir>] [--index <.buzz-index.json>] [--size 1920x1080]
//! ```
//!
//! Writes `<out>/ep1.buzz` (one document scene per script scene), and
//! `<out>/report.md` (per scene: the frames, the lines, who was cast, what was
//! skipped, any warnings). With `--mp4`, one preview per scene is rendered
//! through the same headless render the command line uses
//! (`buzz_app::headless::render`) into `<out>/previews/`. With `--stills N`, N
//! evenly spaced PNGs per scene land in `<out>/stills/`, for looking at without
//! a video player; `--frames 400,600` asks for those frames of every scene
//! instead (a frame past a scene's end is skipped).
//!
//! The screenplay and casting formats are in `buzz_app::episode` (and the
//! screenplay's in `buzz_act::screenplay`). Without audio, timing comes from
//! the words, and so does the lip sync. `--audio <file>` gives the dialogue
//! recording of the chosen scenes, in script order; `--audio-dir <dir>` one file
//! per scene named `scene<N>.*`. The lines are found in it
//! (`buzz_act::audio_align`; Whisper's transcript helps when it is installed,
//! `--no-whisper` skips it), each scene gets its cut on a sound layer and the
//! mouths follow the recording; `<out>/audio_lines.tsv` lists where each line
//! was found. Lines and scenes with no audio keep their text timing.
//!
//! `--scenes` takes the script's own scene numbers (`4,5,6`, or `4-6`); leave it
//! out for every scene. `--assets` points at an asset library other than the
//! user's. `--index` reads the asset index the sets are dressed from (AS3)
//! from that file (a copy, say) instead of the library's own.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use buzz_act::audio_align::{self, AudioPlan, Heard};
use buzz_act::screenplay::{Screenplay, ScriptScene};
use buzz_app::episode::{BuildOptions, Casting, build_episode, report_markdown, scene_file_stem};
use buzz_app::headless::{RenderJob, render};
use buzz_doc::{AssetLibrary, Document};
use buzz_render::GpuPreference;

struct Args {
    script: PathBuf,
    casting: PathBuf,
    scenes: Option<Vec<u32>>,
    out: PathBuf,
    mp4: bool,
    stills: u32,
    frames: Vec<u32>,
    height: u32,
    assets: Option<PathBuf>,
    index: Option<PathBuf>,
    size: Option<(f64, f64)>,
    audio: Option<PathBuf>,
    audio_dir: Option<PathBuf>,
    no_whisper: bool,
}

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

fn parse_args() -> Result<Args> {
    let mut args = std::env::args().skip(1);
    let mut script = None;
    let mut casting = None;
    let mut scenes = None;
    let mut out = None;
    let mut mp4 = false;
    let mut stills = 0u32;
    let mut frames: Vec<u32> = Vec::new();
    let mut height = 540u32;
    let mut assets = None;
    let mut index = None;
    let mut size = None;
    let mut audio = None;
    let mut audio_dir = None;
    let mut no_whisper = false;
    while let Some(flag) = args.next() {
        let mut value = |what: &str| -> Result<String> {
            args.next().with_context(|| format!("{what} needs a value"))
        };
        match flag.as_str() {
            "--script" => script = Some(PathBuf::from(value("--script")?)),
            "--casting" => casting = Some(PathBuf::from(value("--casting")?)),
            "--scenes" => scenes = Some(parse_scenes(&value("--scenes")?)?),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--mp4" => mp4 = true,
            "--stills" => stills = value("--stills")?.parse()?,
            "--frames" => {
                for f in value("--frames")?.split(',').filter(|f| !f.trim().is_empty()) {
                    frames.push(f.trim().parse()?);
                }
            }
            "--height" => height = value("--height")?.parse()?,
            "--assets" => assets = Some(PathBuf::from(value("--assets")?)),
            "--index" => index = Some(PathBuf::from(value("--index")?)),
            "--audio" => audio = Some(PathBuf::from(value("--audio")?)),
            "--audio-dir" => audio_dir = Some(PathBuf::from(value("--audio-dir")?)),
            "--no-whisper" => no_whisper = true,
            "--size" => {
                let text = value("--size")?;
                let (w, h) = text
                    .split_once(['x', 'X'])
                    .with_context(|| "--size is WIDTHxHEIGHT, like 1920x1080")?;
                size = Some((w.trim().parse()?, h.trim().parse()?));
            }
            other => bail!("unknown argument {other:?}"),
        }
    }
    Ok(Args {
        script: script.context("--script <script.txt> is required")?,
        casting: casting.context("--casting <casting.json> is required")?,
        scenes,
        out: out.context("--out <dir> is required")?,
        mp4,
        stills,
        frames,
        height,
        assets,
        index,
        size,
        audio,
        audio_dir,
        no_whisper,
    })
}

/// Whisper's transcript of `clip`, as the aligner's [`Heard`], when a model is
/// installed and it runs.
fn transcript(clip: &buzz_audio::Clip, skip: bool) -> Option<Vec<Heard>> {
    if skip {
        return None;
    }
    if let Err(e) = buzz_ai::transcribe::installed_whisper() {
        eprintln!("No transcript: {e}");
        return None;
    }
    match buzz_ai::transcribe::transcribe_clip(clip, &mut |_, _| {}, &|| false) {
        Ok(t) => {
            eprintln!("Transcript ({}): {} segment(s)", t.provider, t.segments.len());
            Some(
                t.segments
                    .into_iter()
                    .map(|s| Heard {
                        start: s.start,
                        end: s.end,
                        text: s.text,
                    })
                    .collect(),
            )
        }
        Err(e) => {
            eprintln!("No transcript: {e}");
            None
        }
    }
}

/// The audio plan for `--audio` / `--audio-dir`.
fn audio_plan(args: &Args, play: &Screenplay, numbers: &[u32]) -> Result<Option<AudioPlan>> {
    let scenes: Vec<&ScriptScene> = numbers.iter().filter_map(|n| play.scene(*n)).collect();
    if let Some(path) = &args.audio {
        let clip = buzz_audio::Clip::open(path).with_context(|| format!("opening {}", path.display()))?;
        let heard = transcript(&clip, args.no_whisper);
        return Ok(Some(audio_align::plan(&scenes, path, &clip, heard.as_deref())));
    }
    let Some(dir) = &args.audio_dir else {
        return Ok(None);
    };
    let mut plan = AudioPlan {
        path: dir.clone(),
        ..AudioPlan::default()
    };
    for scene in scenes {
        let wanted = format!("scene{}", scene.number);
        let file = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| {
                p.is_file()
                    && p.file_stem()
                        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&wanted))
            });
        let Some(file) = file else {
            plan.notes.push(format!("scene {}: no {wanted}.* in {}; text timing", scene.number, dir.display()));
            continue;
        };
        let clip = buzz_audio::Clip::open(&file).with_context(|| format!("opening {}", file.display()))?;
        let heard = transcript(&clip, args.no_whisper);
        plan.merge(audio_align::plan(&[scene], &file, &clip, heard.as_deref()));
    }
    Ok(Some(plan))
}

fn main() -> Result<()> {
    let args = parse_args()?;
    std::fs::create_dir_all(&args.out).with_context(|| format!("creating {}", args.out.display()))?;

    let source = std::fs::read_to_string(&args.script)
        .with_context(|| format!("reading {}", args.script.display()))?;
    let play = Screenplay::parse(&source);
    if play.scenes.is_empty() {
        bail!("{} has no scenes in it", args.script.display());
    }
    let casting_text = std::fs::read_to_string(&args.casting)
        .with_context(|| format!("reading {}", args.casting.display()))?;
    let casting = Casting::from_json(&casting_text).map_err(anyhow::Error::msg)?;

    let numbers: Vec<u32> = args
        .scenes
        .clone()
        .unwrap_or_else(|| play.scenes.iter().map(|s| s.number).collect());
    let assets = match &args.assets {
        Some(dir) => AssetLibrary::at(dir),
        None => AssetLibrary::user(),
    };

    let mut opts = BuildOptions::default();
    if let Some(path) = &args.index {
        let index = buzz_app::asset_sets::load_index_file(path)
            .with_context(|| format!("{}: not an asset index with records", path.display()))?;
        eprintln!("Asset index {}: {} records", path.display(), index.records.len());
        opts.index = Some(std::sync::Arc::new(index));
    }
    if let Some((w, h)) = args.size {
        opts.width = w;
        opts.height = h;
    }
    if let Some(plan) = audio_plan(&args, &play, &numbers)? {
        for note in &plan.notes {
            eprintln!("Audio: {note}");
        }
        let mut tsv = String::from("scene\titem\tstart\tend\n");
        for l in &plan.lines {
            tsv.push_str(&format!("{}\t{}\t{:.3}\t{:.3}\n", l.scene, l.item, l.start, l.end));
        }
        let path = args.out.join("audio_lines.tsv");
        std::fs::write(&path, tsv).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("Wrote {}", path.display());
        opts.audio = Some(std::sync::Arc::new(plan));
    }

    eprintln!(
        "{}: {} scene(s) in the script; building {:?} from {} asset(s)",
        play.title,
        play.scenes.len(),
        numbers,
        assets.len()
    );
    let (mut doc, reports) = build_episode(&play, &casting, &assets, &numbers, &opts);

    let buzz = args.out.join("ep1.buzz");
    doc.save_as(&buzz)
        .with_context(|| format!("saving {}", buzz.display()))?;
    eprintln!("Saved {}", buzz.display());

    let report_path = args.out.join("report.md");
    let title = if play.title.is_empty() { "episode" } else { play.title.as_str() };
    std::fs::write(&report_path, report_markdown(title, &reports, opts.fps))
        .with_context(|| format!("writing {}", report_path.display()))?;
    eprintln!("Wrote {}", report_path.display());

    for r in &reports {
        eprintln!(
            "  scene {:>2}: {:>5} frames, {} line(s), {} caption(s), cast {}{}",
            r.number,
            r.frames,
            r.lines,
            r.captions,
            r.cast.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "),
            if r.skipped.is_empty() {
                String::new()
            } else {
                format!("; skipped {}", r.skipped.len())
            }
        );
    }

    // P5: the camera's cuts, printed and saved as `<out>/shots.json`.
    if reports.iter().any(|r| !r.shots.is_empty()) {
        let mut cuts = Vec::new();
        for r in &reports {
            for s in &r.shots {
                println!(
                    "scene {} frame {:>5}: {:<6} {:<8} camera ({:.0}, {:.0}) x{:.3}{}",
                    r.number,
                    s.frame,
                    s.kind,
                    s.on.as_deref().unwrap_or("-"),
                    s.x,
                    s.y,
                    s.scale,
                    s.push_to.map(|f| format!(" push 3% to {f}")).unwrap_or_default()
                );
                cuts.push(serde_json::json!({
                    "scene": r.number,
                    "frame": s.frame,
                    "kind": s.kind,
                    "on": s.on,
                    "camera": {"x": s.x, "y": s.y, "scale": s.scale},
                    "push_to": s.push_to,
                }));
            }
        }
        let path = args.out.join("shots.json");
        std::fs::write(&path, serde_json::to_string_pretty(&serde_json::json!({ "cuts": cuts }))?)
            .with_context(|| format!("writing {}", path.display()))?;
        eprintln!("Wrote {}", path.display());
    }

    if args.mp4 || args.stills > 0 || !args.frames.is_empty() {
        let names = doc.scene_names();
        let scenes = doc.film();
        // The scenes that were built, in the order they sit in the document.
        let built: Vec<u32> = reports
            .iter()
            .filter(|r| r.frames > 0)
            .map(|r| r.number)
            .collect();
        for (index, scene) in scenes.iter().enumerate() {
            let number = built.get(index).copied().unwrap_or(index as u32 + 1);
            let title = reports
                .iter()
                .find(|r| r.number == number)
                .map(|r| r.title.clone())
                .unwrap_or_default();
            let stem = scene_file_stem(number, &title);
            let name = names.get(index).cloned().unwrap_or_else(|| stem.clone());

            if args.stills > 0 || !args.frames.is_empty() {
                stills(
                    scene,
                    &args.out.join("stills"),
                    &stem,
                    args.stills,
                    &args.frames,
                    args.height,
                )?;
            }
            if args.mp4 {
                preview(scene.clone(), &name, &args.out.join("previews"), &stem, args.height)?;
            }
        }
    }
    Ok(())
}

/// One scene's preview, through the headless render.
fn preview(scene: buzz_scene::Scene, name: &str, dir: &Path, stem: &str, height: u32) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    // The render reads a document from disk, so each scene is saved alone into
    // a temporary one: the film it makes is this scene and no other.
    let temp = tempfile::tempdir()?;
    let mut solo = Document::from_scenes(vec![(name.to_string(), scene)]);
    let path = temp.path().join(format!("{stem}.buzz"));
    solo.save_as(&path)?;

    let output = dir.join(format!("{stem}.mp4"));
    let started = std::time::Instant::now();
    let message = render(&RenderJob {
        document: Some(path),
        brief: None,
        script: None,
        audio: Vec::new(),
        save: None,
        preview: None,
        output: Some(output.clone()),
        height: Some(height),
        gpu: GpuPreference::Automatic,
    })
    .with_context(|| format!("rendering {}", output.display()))?;
    eprintln!(
        "Rendered {} in {:.1}s: {message}",
        output.display(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// `count` evenly spaced stills from a scene, and any frames asked for by number.
fn stills(
    scene: &buzz_scene::Scene,
    dir: &Path,
    stem: &str,
    count: u32,
    wanted: &[u32],
    height: u32,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut settings = buzz_export::ExportSettings::for_stage(scene);
    let aspect = settings.width as f64 / settings.height.max(1) as f64;
    settings.height = height + height % 2;
    settings.width = ((height as f64 * aspect).round().max(2.0) as u32 + 1) & !1;
    let mut exporter = buzz_export::Exporter::new(&GpuPreference::Automatic)?;
    let frames = scene.frame_count().max(1);
    let mut picks: Vec<u32> = (0..count)
        .map(|i| (((i as f64 + 0.5) / count as f64 * frames as f64) as u32).min(frames - 1))
        .collect();
    picks.extend(wanted.iter().copied().filter(|f| *f < frames));
    for frame in picks {
        let rendered = exporter.render(scene, frame, &settings)?;
        let path = dir.join(format!("{stem}_f{frame:04}.png"));
        rendered.write_png(&path)?;
        eprintln!("Wrote {}", path.display());
    }
    Ok(())
}
