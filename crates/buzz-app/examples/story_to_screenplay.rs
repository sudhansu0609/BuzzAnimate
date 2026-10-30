//! **A story in, the producer's screenplay out** (ANIMATE_COPILOT_PLAN B1).
//!
//! ```text
//! cargo run --release -p buzz-app --example story_to_screenplay -- \
//!     --story story.txt --series Chirailiya --out out_dir [--assets <asset library>]
//! ```
//! Writes `<out>/screenplay.md` and `<out>/cast.json`; the series' cast registry
//! lives at `<assets>/Cast/<series>/cast_registry.json`.
use std::path::PathBuf;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn main() -> anyhow::Result<()> {
    let story = PathBuf::from(arg("--story").ok_or_else(|| anyhow::anyhow!("--story <txt> is required"))?);
    let out = PathBuf::from(arg("--out").ok_or_else(|| anyhow::anyhow!("--out <dir> is required"))?);
    let series = arg("--series").unwrap_or_default();
    let assets = match arg("--assets") {
        Some(a) => PathBuf::from(a),
        None => buzz_doc::AssetLibrary::user().root().map(|p| p.to_path_buf()).ok_or_else(|| anyhow::anyhow!("no asset library folder; pass --assets"))?,
    };
    buzz_ai::llm::set_low_reasoning(true);
    let llm = buzz_ai::story::Endpoints::standard();
    let r = buzz_ai::story::story_to_screenplay(&llm, "", &story, &series, &out, &assets, &mut |l| println!("{l}"), &|| false)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for c in &r.cast {
        println!("cast: {} ({}) {}", c.name, c.id, c.role);
    }
    println!("screenplay: {}\ncast: {}", r.screenplay.display(), r.cast_path.display());
    Ok(())
}
