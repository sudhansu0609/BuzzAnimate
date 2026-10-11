//! Re-save a `.buzz` file, timing the load and the save, and check the copy
//! reads back identical. Also reports how many rig-part artworks the loaded
//! scene holds as separate allocations (the in-memory duplication).
//!
//! ```text
//! cargo run --release -p buzz-doc --example resave -- <in.buzz> <out.buzz>
//! ```
use std::collections::HashSet;
use std::time::Instant;

use buzz_scene::{ObjectKind, Scene};

fn rig_stats(scene: &Scene) -> (usize, usize, usize) {
    let (mut parts, mut distinct, mut points) = (0usize, HashSet::new(), 0usize);
    let mut layers: Vec<&std::sync::Arc<buzz_scene::Layer>> = scene.stage_layers().iter().collect();
    for symbol in scene.library().iter() {
        layers.extend(symbol.layers.iter());
    }
    for layer in layers {
        for key in layer.frames.keyframes() {
            for object in key.objects.iter() {
                if let ObjectKind::Armature(rig) = &object.kind {
                    for part in &rig.parts {
                        parts += 1;
                        distinct.insert(std::sync::Arc::as_ptr(&part.artwork) as usize);
                        if let buzz_scene::RigBinding::Skin(skin) = &part.binding {
                            points += skin.points.len();
                        }
                    }
                }
            }
        }
    }
    (parts, distinct.len(), points)
}

/// Whether two scenes hold the same layers and library; when they do not,
/// says where the first difference is. A JSON number read back can differ from
/// the one written in its last bit (serde_json's default float parser), so a
/// difference that is only a digit at the end of a long number is noise of
/// that kind, not a lost value.
fn same(a: &Scene, b: &Scene) -> bool {
    let mut layers_a: Vec<&std::sync::Arc<buzz_scene::Layer>> = a.stage_layers().iter().collect();
    let mut layers_b: Vec<&std::sync::Arc<buzz_scene::Layer>> = b.stage_layers().iter().collect();
    for symbol in a.library().iter() {
        layers_a.extend(symbol.layers.iter());
    }
    for symbol in b.library().iter() {
        layers_b.extend(symbol.layers.iter());
    }
    if layers_a.len() != layers_b.len() {
        println!("  layer counts differ: {} vs {}", layers_a.len(), layers_b.len());
        return false;
    }
    let mut same = true;
    let mut reported = 0;
    for (li, (la, lb)) in layers_a.iter().zip(&layers_b).enumerate() {
        if la == lb {
            continue;
        }
        same = false;
        let (ka, kb) = (la.frames.keyframes(), lb.frames.keyframes());
        if ka.len() != kb.len() {
            println!("  layer {li}: {} keyframes vs {}", ka.len(), kb.len());
            continue;
        }
        for (ki, (xa, xb)) in ka.iter().zip(kb).enumerate() {
            if xa == xb || reported >= 3 {
                continue;
            }
            reported += 1;
            let (da, db) = (format!("{:?}", xa.objects), format!("{:?}", xb.objects));
            let at = da
                .bytes()
                .zip(db.bytes())
                .position(|(x, y)| x != y)
                .unwrap_or(da.len().min(db.len()));
            let lo = at.saturating_sub(60);
            println!(
                "  layer {li} key {ki}: first difference at byte {at}
    a: ...{}
    b: ...{}",
                &da[lo..(at + 60).min(da.len())],
                &db[lo..(at + 60).min(db.len())]
            );
        }
    }
    same
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("input .buzz");
    let output = args.next().expect("output .buzz");

    let t = Instant::now();
    let scene = buzz_doc::format::load(&input).expect("load input");
    println!("load input   {:>8} ms", t.elapsed().as_millis());
    let (parts, distinct, points) = rig_stats(&scene);
    println!(
        "rig parts in memory: {parts} (distinct artwork allocations {distinct}), skin weight rows {points}"
    );

    let t = Instant::now();
    buzz_doc::format::save(&scene, &output).expect("save");
    println!("save         {:>8} ms", t.elapsed().as_millis());
    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    println!("file         {:>8.2} MB", size as f64 / 1_048_576.0);

    let t = Instant::now();
    let back = buzz_doc::format::load(&output).expect("load output");
    println!("load output  {:>8} ms", t.elapsed().as_millis());
    let (parts, distinct, _) = rig_stats(&back);
    println!("rig parts after reload: {parts} (distinct artwork allocations {distinct})");
    println!("identical    {}", same(&scene, &back));
}
