//! alaxmi_render --doc part_01.buzz --out part_01.mp4 [--height 540]: render a .buzz to a film.
use anyhow::{Context, Result, bail};
use buzz_app::headless::{RenderJob, render};
use buzz_render::GpuPreference;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut doc, mut out, mut height) = (None, None, 540u32);
    while let Some(flag) = args.next() {
        let mut value = |what: &str| args.next().with_context(|| format!("{what} needs a value"));
        match flag.as_str() {
            "--doc" => doc = Some(PathBuf::from(value("--doc")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--height" => height = value("--height")?.parse()?,
            other => bail!("unknown flag {other}"),
        }
    }
    let report = render(&RenderJob {
        document: Some(doc.context("--doc")?),
        brief: None,
        script: None,
        audio: Vec::new(),
        save: None,
        preview: None,
        output: Some(out.context("--out")?),
        height: Some(height),
        gpu: GpuPreference::Automatic,
    })?;
    println!("{report}");
    Ok(())
}
