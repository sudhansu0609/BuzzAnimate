//! alaxmi_retrace: split, cut objects, trace and re-save ONE background from its picture on disk, as the
//! producer's backgrounds step does after drawing it (no ComfyUI).
//!
//!   alaxmi_retrace --picture backgrounds/village_s5-93586f83.png --key village_s5 --assets <library root>
//!       [--series Khayal3Baje] [--no-objects]
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use buzz_app::backdrop::{self, BackdropOptions};
use buzz_doc::AssetLibrary;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut picture, mut key, mut assets, mut series, mut objects) = (None, None, None, "Khayal3Baje".to_string(), true);
    while let Some(flag) = args.next() {
        let mut value = |what: &str| args.next().with_context(|| format!("{what} needs a value"));
        match flag.as_str() {
            "--picture" => picture = Some(PathBuf::from(value("--picture")?)),
            "--key" => key = Some(value("--key")?),
            "--assets" => assets = Some(PathBuf::from(value("--assets")?)),
            "--series" => series = value("--series")?,
            "--no-objects" => objects = false,
            other => bail!("unknown flag {other}"),
        }
    }
    let (picture_path, key, assets) = (picture.context("--picture")?, key.context("--key")?, assets.context("--assets")?);
    buzz_app::comfy_profile::set_series(&series);
    let picture = image::open(&picture_path).with_context(|| picture_path.display().to_string())?.to_rgba8();
    let plates = backdrop::split_plates(&picture);
    let style = backdrop::Style::painterly();
    let opts = BackdropOptions::for_style(&style);
    let cut = objects.then(|| {
        eprintln!("cutting objects...");
        buzz_app::bg_objects::extract_objects(&picture, &plates, &buzz_app::bg_objects::ObjectOptions { trace: opts, ..Default::default() })
    });
    let (plates, compare_with) = match &cut {
        Some(c) => (
            backdrop::Plates { sky: c.filled_plates.sky.clone(), far: c.filled_plates.far.clone(), mid: c.filled_plates.mid.clone(), ground: c.filled_plates.ground.clone(), horizon: c.filled_plates.horizon },
            c.picture_without_objects(&picture),
        ),
        None => (plates, picture.clone()),
    };
    let mut render_check = buzz_app::definition::RenderCheck::default();
    let checked = buzz_app::definition::trace_checked(&plates, &compare_with, &opts, &mut render_check, &mut |l| eprintln!("{l}"));
    let mut traced = checked.scene;
    let mut library = AssetLibrary::at(&assets);
    let empty = buzz_app::bg_objects::ObjectCut {
        objects: Vec::new(),
        filled_plates: backdrop::Plates { sky: plates.sky.clone(), far: plates.far.clone(), mid: plates.mid.clone(), ground: plates.ground.clone(), horizon: plates.horizon },
        notes: Vec::new(),
    };
    let path = format!("Backgrounds/{series}/{key}");
    let (asset, placed) = backdrop::save_asset_with_objects(&mut traced, cut.as_ref().unwrap_or(&empty), &key, (picture.width(), picture.height()), &mut library, &path, Some(&picture_path))
        .map_err(|e| anyhow!(e))?;
    println!("{} saved: {} objects, trace score {:?}", asset.path.display(), placed.len(), checked.score.map(|s| (s.mae, s.ssim)));
    Ok(())
}
