//! P2 verification helper (`docs/IMPORT_PERF_PLAN.md`): compare two PNGs of
//! the same size and report how many pixels differ by more than a channel
//! threshold, plus the worst single-channel difference found.
//!
//! cargo run --release -p buzz-app --example png_diff -- a.png b.png [threshold]
//!
//! Not part of the render walk itself — a standalone diagnostic, the same
//! role `stage_cost.rs` and `ai_verify.rs` already play in this directory.

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let a_path = args.next().ok_or_else(|| anyhow::anyhow!("missing image a"))?;
    let b_path = args.next().ok_or_else(|| anyhow::anyhow!("missing image b"))?;
    let threshold: u8 = args
        .next()
        .map(|v| v.parse::<u32>())
        .transpose()?
        .unwrap_or(40)
        .min(255) as u8;

    let a = image::open(&a_path)?.to_rgba8();
    let b = image::open(&b_path)?.to_rgba8();

    if a.dimensions() != b.dimensions() {
        anyhow::bail!(
            "size mismatch: {a_path} is {:?}, {b_path} is {:?}",
            a.dimensions(),
            b.dimensions()
        );
    }

    let mut over = 0u64;
    let mut max_diff = 0u8;
    let mut sum_diff = 0u64;
    let total_pixels = a.pixels().len() as u64;

    for (pa, pb) in a.pixels().zip(b.pixels()) {
        let mut worst = 0u8;
        for c in 0..4 {
            let d = pa.0[c].abs_diff(pb.0[c]);
            worst = worst.max(d);
            sum_diff += u64::from(d);
        }
        max_diff = max_diff.max(worst);
        if worst > threshold {
            over += 1;
        }
    }

    let identical_pct = 100.0 * (1.0 - over as f64 / total_pixels as f64);
    println!(
        "{a_path} vs {b_path}: {} x {} px, threshold {threshold}",
        a.width(),
        a.height()
    );
    println!(
        "  pixels over threshold: {over} / {total_pixels} ({:.4}% identical)",
        identical_pct
    );
    println!("  worst single-channel diff: {max_diff}");
    println!(
        "  mean per-channel diff: {:.4}",
        sum_diff as f64 / (total_pixels * 4) as f64
    );
    Ok(())
}
