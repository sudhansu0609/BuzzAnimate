//! A background from ComfyUI, split into depth plates and traced to vectors
//! (PRODUCER_PLAN § P3).
//!
//! cargo run --release -p buzz-app --example backdrop -- --prompt "<text>" --out <dir>
//!     [--seed N] [--style inked|comic|painterly] [--launch] [--bitmap-only] [--no-people-check]
//!     [--assets <library dir>] [--series Name] [--key name]
//!     [--colours N] [--smoothness F] [--speckle N] [--bold F] [--ink true|false]
//!
//! Each generated picture is looked at by DWPose; one with a person 8 % of
//! its height or taller is drawn again with the next seed, up to 3 tries
//! (`--no-people-check` turns that off).
//!
//! Writes `<out>/bg.png` (cached: an existing one is reused), `plates/{sky,far,mid,ground}.png`,
//! `traced.png` (the traced asset at 1920×1080), `compare.png` (bg | traced at
//! half size) and `pan.png` (3 frames of a slow camera pan, stacked). The asset
//! goes to `<assets>/Backgrounds/<series>/<key>` — `--assets` defaults to
//! `<out>/assets`, never the user's library.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use buzz_app::backdrop::{self, BackdropOptions, Style};
use buzz_app::comfy::ComfyClient;
use buzz_scene::{CameraKey, Scene};
use image::{RgbaImage, imageops};

fn render(exporter: &mut buzz_export::Exporter, scene: &Scene, frame: u32) -> anyhow::Result<RgbaImage> {
    let settings = buzz_export::ExportSettings::for_stage(scene);
    let f = exporter.render(scene, frame, &settings)?;
    RgbaImage::from_raw(f.width, f.height, f.pixels).ok_or_else(|| anyhow!("bad frame size"))
}

fn half(img: &RgbaImage) -> RgbaImage {
    imageops::resize(img, img.width() / 2, img.height() / 2, imageops::FilterType::Triangle)
}

fn main() -> anyhow::Result<()> {
    let mut prompt = None;
    let mut out: Option<PathBuf> = None;
    let mut seed: u64 = 7;
    let mut style = Style::inked();
    let mut launch = false;
    let mut bitmap_only = false;
    let mut assets: Option<PathBuf> = None;
    let mut series = "Originals".to_string();
    let mut key: Option<String> = None;
    let mut colours = None;
    let mut smoothness = None;
    let mut speckle = None;
    let mut bold = None;
    let mut ink: Option<bool> = None;
    let mut people_check = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow!("{a} needs a value"));
        match a.as_str() {
            "--prompt" => prompt = Some(value()?),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--seed" => seed = value()?.parse()?,
            "--style" => {
                let v = value()?;
                style = Style::by_name(&v).ok_or_else(|| anyhow!("unknown style {v}"))?;
            }
            "--launch" => launch = true,
            "--no-people-check" => people_check = false,
            "--bitmap-only" => bitmap_only = true,
            "--assets" => assets = Some(PathBuf::from(value()?)),
            "--series" => series = value()?,
            "--key" => key = Some(value()?),
            "--colours" => colours = Some(value()?.parse()?),
            "--smoothness" => smoothness = Some(value()?.parse()?),
            "--speckle" => speckle = Some(value()?.parse()?),
            "--bold" => bold = Some(value()?.parse()?),
            "--ink" => ink = Some(value()?.parse()?),
            other => return Err(anyhow!("unknown argument {other}")),
        }
    }
    let prompt = prompt.ok_or_else(|| anyhow!("--prompt is required"))?;
    let out = out.ok_or_else(|| anyhow!("--out is required"))?;
    std::fs::create_dir_all(out.join("plates"))?;
    let key = key.unwrap_or_else(|| {
        out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "background".into())
    });

    // 1. Generate (cached).
    let bg_path = out.join("bg.png");
    if bg_path.is_file() {
        println!("bg.png exists; reused");
    } else {
        let t = Instant::now();
        let client = ComfyClient::ensure_running(launch, Duration::from_secs(180)).map_err(anyhow::Error::msg)?;
        println!("ComfyUI at {} ({:.1} s to reach)", client.base_url, t.elapsed().as_secs_f64());
        println!(
            "workflow: {}",
            backdrop::workflow_path().map(|p| p.display().to_string()).unwrap_or_else(|| "none".into())
        );
        let full = backdrop::background_prompt(&prompt, &style);
        println!("prompt: {full}");
        let t = Instant::now();
        let (_, tries) = backdrop::generate_checked(Some(&client), &full, seed, &bg_path, 3, people_check, &mut |l| {
            println!("  {l}")
        })
        .map_err(anyhow::Error::msg)?;
        println!("people check: {}", tries.iter().enumerate().map(|(i, t)| t.describe(i + 1)).collect::<Vec<_>>().join(", "));
        println!("generated in {:.1} s", t.elapsed().as_secs_f64());
    }
    if bitmap_only {
        return Ok(());
    }
    let bg = image::open(&bg_path).with_context(|| bg_path.display().to_string())?.to_rgba8();
    println!("bg {}x{}", bg.width(), bg.height());

    // 2. Plates.
    let t = Instant::now();
    let plates = backdrop::split_plates(&bg);
    println!("plates in {:.1} s; horizon row {}", t.elapsed().as_secs_f64(), plates.horizon);
    for (name, plate) in [("sky", &plates.sky), ("far", &plates.far), ("mid", &plates.mid), ("ground", &plates.ground)] {
        let opaque = plate.pixels().filter(|p| p.0[3] > 0).count();
        println!("  {name}: {:.1} % of the picture", 100.0 * opaque as f64 / (plate.width() * plate.height()) as f64);
        plate.save(out.join("plates").join(format!("{name}.png")))?;
    }

    // 3. Trace.
    let mut opts = BackdropOptions::for_style(&style);
    if let Some(v) = colours {
        opts.colours = v;
    }
    if let Some(v) = smoothness {
        opts.smoothness = v;
    }
    if let Some(v) = speckle {
        opts.speckle = v;
    }
    if let Some(v) = bold {
        opts.bold = v;
    }
    if let Some(v) = ink {
        opts.ink = v;
    }
    println!("{opts:?}");
    let fit = backdrop::fit_sky(&plates.sky);
    println!("sky fit: ramp {} stops, glows {:?} stops, {} residual parts", fit.stops, fit.glow_stops, fit.parts);
    let t = Instant::now();
    let mut scene = backdrop::trace_plates(&plates, &opts);
    println!("traced in {:.1} s", t.elapsed().as_secs_f64());

    // 4. Save the asset (scratch library unless told otherwise).
    let library_dir = assets.unwrap_or_else(|| out.join("assets"));
    let mut library = buzz_doc::AssetLibrary::at(&library_dir);
    let asset = backdrop::save_asset(&scene, &mut library, &format!("Backgrounds/{series}/{key}"), Some(&bg_path))
        .map_err(anyhow::Error::msg)?;
    println!("asset {}", asset.path.display());

    // 5. Previews.
    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::default())?;
    let traced = render(&mut exporter, &scene, 0)?;
    traced.save(out.join("traced.png"))?;
    let (a, b) = (half(&bg), half(&traced));
    let mut compare = RgbaImage::new(a.width() + b.width(), a.height().max(b.height()));
    imageops::overlay(&mut compare, &a, 0, 0);
    imageops::overlay(&mut compare, &b, a.width() as i64, 0);
    compare.save(out.join("compare.png"))?;

    // A slow pan, zoomed in a little so the frame never leaves the stage.
    scene.set_frame_count(3);
    let (cw, ch) = (backdrop::STAGE.0 / 2.0, backdrop::STAGE.1 / 2.0);
    let camera = scene.camera_mut();
    camera.enabled = true;
    for (frame, dx) in [(0u32, -80.0), (2, 80.0)] {
        let mut k = CameraKey::new(frame, buzz_geom::Point::new(cw + dx, ch));
        k.zoom = 1.1;
        camera.set_key(k);
    }
    let frames: Vec<RgbaImage> = (0..3).map(|f| render(&mut exporter, &scene, f).map(|i| half(&i))).collect::<Result<_, _>>()?;
    let mut pan = RgbaImage::new(frames[0].width(), frames.iter().map(|f| f.height()).sum());
    let mut y = 0;
    for f in &frames {
        imageops::overlay(&mut pan, f, 0, y);
        y += f.height() as i64;
    }
    pan.save(out.join("pan.png"))?;
    println!("wrote {}", Path::new(&out).display());
    Ok(())
}
