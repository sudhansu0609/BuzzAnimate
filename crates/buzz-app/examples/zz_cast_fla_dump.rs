//! scratch probe: list an FLA's stage and symbol tree — delete after use.
use buzz_scene::{ObjectKind, Scene, SymbolId};
use std::collections::BTreeSet;

fn walk(scene: &Scene, id: SymbolId, depth: usize, seen: &mut BTreeSet<SymbolId>, max: usize) {
    let Some(s) = scene.library().get(id) else { return };
    let mut labels = Vec::new();
    let mut keys = 0;
    for l in s.layers.iter() {
        for k in l.frames.keyframes() {
            keys += 1;
            if let Some(lb) = &k.label { labels.push(format!("{}@{}", lb, k.start)); }
        }
    }
    println!("{}{} [{}] len={} layers={} keys={} labels={:?}", "  ".repeat(depth), s.name, s.folder.as_deref().unwrap_or(""), s.length(), s.layers.iter().count(), keys, labels);
    if depth >= max || !seen.insert(id) { return; }
    let mut kids = BTreeSet::new();
    for l in s.layers.iter() {
        for k in l.frames.keyframes() {
            for o in k.objects.iter() {
                if let ObjectKind::Instance(i) = &o.kind { kids.insert(i.symbol); }
            }
        }
    }
    for k in kids { walk(scene, k, depth + 1, seen, max); }
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let max: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(2);
    let scene = buzz_app::import::read(std::path::Path::new(&path)).expect("import").scene;
    println!("library symbols: {}", scene.library().iter().count());
    let mut seen = BTreeSet::new();
    for l in scene.stage_layers().iter() {
        let keys = l.frames.keyframes();
        println!("STAGE LAYER '{}' keys={}", l.name, keys.len());
        let mut top = BTreeSet::new();
        for k in keys {
            for o in k.objects.iter() {
                if let ObjectKind::Instance(i) = &o.kind {
                    if top.insert(i.symbol) {
                        println!("  instance name={:?} @{}", o.name, k.start);
                        walk(&scene, i.symbol, 2, &mut seen, max + 2);
                    }
                }
            }
        }
    }
}
