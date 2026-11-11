//! E4b: zz_e4b_fla <ep1.buzz> <out.fla> — export the first scene of a produced document to a
//! FLA (never opened in Animate) so its XFL can be inspected: the head symbol's Mouth/Eyelid keys.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0])?;
    let (scenes, _) = buzz_doc::format::from_bytes_scenes(&bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    let report = buzz_export::xfl::export_fla(&scenes[0].1, &a[1]).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{report:?}");
    Ok(())
}
