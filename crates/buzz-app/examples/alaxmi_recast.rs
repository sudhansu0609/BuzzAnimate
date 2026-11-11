//! alaxmi_recast: the Producer panel's "recast" without the panel, for a produced episode folder.
//!
//!   alaxmi_recast --out <produce dir> --member DEEPAK --asset Raju --scenes 1-4 [--move "2:0.3:0.9"]
//!
//! `--move scene:x:y` first stands the member at x,y (fractions of the stage) in that scene, the way a hand
//! edit does; then each scene in `--scenes` is recast and rebuilt on its own.
use anyhow::{Context, Result, bail};
use buzz_app::copilot::{self, RealRebuilder, Run};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut out, mut member, mut asset, mut scenes, mut mv) = (None, None, None, None, None);
    while let Some(flag) = args.next() {
        let mut value = |what: &str| args.next().with_context(|| format!("{what} needs a value"));
        match flag.as_str() {
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--member" => member = Some(value("--member")?),
            "--asset" => asset = Some(value("--asset")?),
            "--scenes" => scenes = Some(value("--scenes")?),
            "--move" => mv = Some(value("--move")?),
            other => bail!("unknown flag {other}"),
        }
    }
    let out = out.context("--out")?;
    let member = member.context("--member")?;
    let asset = asset.context("--asset")?;
    let range = scenes.context("--scenes a-b")?;
    let (a, b) = range.split_once('-').context("--scenes a-b")?;
    let (a, b): (u32, u32) = (a.parse()?, b.parse()?);
    let mut rb = RealRebuilder::default();
    let mut say = |l: &str| println!("{l}");
    let cancel = || false;
    let mut run = Run { out: &out, rb: &mut rb, say: &mut say, cancel: &cancel };
    if let Some(m) = mv {
        let p: Vec<&str> = m.split(':').collect();
        let r = copilot::move_character(&mut run, p[0].parse()?, 0, &member, p[1].parse()?, p[2].parse()?)?;
        println!("{}", r.changed);
    }
    for n in a..=b {
        let r = copilot::recast(&mut run, &member, &asset, Some(n))?;
        println!("{} (scenes {:?}, stills {:?})", r.changed, r.scenes, r.stills);
    }
    Ok(())
}
