//! hair_flow_apply: put a live Hair Flow modifier on the artwork of named layers of a .buzz.
//!
//!   hair_flow_apply <asset.buzz> --layers "Hair Back,Hair Front" [--preset eerie|breeze|gale]
//!                   [--root top|bottom|left|right|pivot] [--out other.buzz] [--remove]
//!
//! Every object on each named layer (every keyframe) gets the modifier; one that already has a Hair Flow has it
//! replaced, so running this twice does not stack. `--remove` takes Hair Flow off instead. Without `--out` the
//! file is rewritten in place. Layer names match case-insensitively and exactly; the layers must be in the
//! document's own stack (a Cast asset's character layers are).
//!
//! Presets (tip = share of hair length, Hz, wave = share of length, wind degrees, lag seconds):
//!   eerie   tip 0.10  0.18 Hz  wave 0.8  wind -75  lag 0.30   still air, a slow lift
//!   breeze  tip 0.08  0.25 Hz  wave 0.6  wind -20  lag 0.18
//!   gale    tip 0.18  0.60 Hz  wave 0.4  wind   0  lag 0.12
use anyhow::{Context, Result, bail};
use buzz_doc::Document;
use buzz_scene::{Modifier, ObjectId};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let asset = PathBuf::from(args.next().context("usage: hair_flow_apply <asset.buzz> --layers \"A,B\"")?);
    let (mut layers, mut preset, mut root, mut out, mut remove) = (None, "eerie".to_string(), 0.0, None, false);
    while let Some(flag) = args.next() {
        let mut value = |what: &str| args.next().with_context(|| format!("{what} needs a value"));
        match flag.as_str() {
            "--layers" => layers = Some(value("--layers")?),
            "--preset" => preset = value("--preset")?,
            "--root" => {
                root = match value("--root")?.to_ascii_lowercase().as_str() {
                    "top" => 0.0,
                    "bottom" => 1.0,
                    "left" => 2.0,
                    "right" => 3.0,
                    "pivot" => 4.0,
                    other => bail!("--root: top, bottom, left, right or pivot, not {other}"),
                }
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--remove" => remove = true,
            other => bail!("unknown flag {other}"),
        }
    }
    let wanted: Vec<String> = layers
        .context("--layers needs a comma-separated list of layer names")?
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let (strength, rate, wavelength, wind, lag) = match preset.as_str() {
        "eerie" => (0.10, 0.18, 0.8, -75.0, 0.30),
        "breeze" => (0.08, 0.25, 0.6, -20.0, 0.18),
        "gale" => (0.18, 0.60, 0.4, 0.0, 0.12),
        other => bail!("--preset: eerie, breeze or gale, not {other}"),
    };
    let modifier = Modifier::HairFlow { strength, rate, wavelength, wind, lag, root };

    let mut doc = Document::open(&asset).with_context(|| asset.display().to_string())?;
    let mut ids: BTreeSet<u64> = BTreeSet::new();
    let mut matched = Vec::new();
    {
        let scene = doc.scene();
        for layer in scene.layers().iter() {
            if !wanted.contains(&layer.name.to_lowercase()) {
                continue;
            }
            matched.push(layer.name.clone());
            for frame in 0..layer.frames.length().max(1) {
                for object in layer.frames.resolved_at(frame).iter() {
                    ids.insert(object.id.0);
                }
            }
        }
    }
    if matched.is_empty() {
        bail!("none of the layers {:?} is in {} (top-level layers only)", wanted, asset.display());
    }
    doc.edit("Hair Flow", |scene| {
        for id in &ids {
            scene.update_object_across(0, u32::MAX, ObjectId(*id), |o| {
                o.modifiers.retain(|m| !matches!(m, Modifier::HairFlow { .. }));
                if !remove {
                    o.modifiers.push(modifier);
                }
            });
        }
    });
    doc.end_gesture();
    let target = out.unwrap_or(asset);
    doc.save_as(&target).with_context(|| target.display().to_string())?;
    println!(
        "{} Hair Flow ({preset}) on {} objects of layers {:?} -> {}",
        if remove { "removed" } else { "applied" },
        ids.len(),
        matched,
        target.display()
    );
    Ok(())
}
