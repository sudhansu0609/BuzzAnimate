//! zz_rig_check file.fla animate_world.tsv — compare every rigged part's
//! imported world matrix at frame 0 with Adobe Animate's own (dumped by
//! detaching each layer in Animate). Per character, the best-matching
//! library symbol and its worst coefficient error.
use std::collections::HashMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let imported = buzz_app::import::read(std::path::Path::new(&a[0])).unwrap();
    let text = std::fs::read_to_string(&a[1]).unwrap();
    let mut jobs: HashMap<u32, Vec<(String, [f64; 6])>> = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 11 || f[0].parse::<u32>().is_err() { continue; }
        let m: Vec<f64> = f[5..11].iter().map(|v| v.parse().unwrap()).collect();
        jobs.entry(f[0].parse().unwrap()).or_default().push((f[3].to_string(), [m[0], m[1], m[2], m[3], m[4], m[5]]));
    }
    let mut keys: Vec<_> = jobs.keys().copied().collect();
    keys.sort();
    for j in keys {
        let want = &jobs[&j];
        let mut best: Option<(f64, String, Vec<String>)> = None;
        for sym in imported.scene.library().iter() {
            let mut layers: HashMap<String, Vec<buzz_scene::LayerId>> = HashMap::new();
            for l in sym.layers.iter() { layers.entry(l.name.clone()).or_default().push(l.id); }
            if !want.iter().all(|(n, _)| layers.contains_key(n)) { continue; }
            let mut used: HashMap<String, usize> = HashMap::new();
            let mut worst = 0.0f64;
            let mut notes = Vec::new();
            for (name, m) in want {
                let k = used.entry(name.clone()).or_insert(0);
                let ids = &layers[name];
                let Some(id) = ids.get(*k).copied() else { continue };
                *k += 1;
                let layer = sym.layers.get(id).unwrap();
                let resolved = layer.frames.resolved_at(0u32);
                let Some(obj) = resolved.iter().next() else { continue };
                let c = obj.transform.as_coeffs();
                let err = (0..6).map(|k| (c[k] - m[k]).abs() / if k < 4 { 0.01 } else { 1.0 }).fold(0.0, f64::max);
                if err > 1.0 { notes.push(format!("{name}: ours [{:.3} {:.3} {:.3} {:.3} {:.1} {:.1}] animate [{:.3} {:.3} {:.3} {:.3} {:.1} {:.1}]", c[0],c[1],c[2],c[3],c[4],c[5], m[0],m[1],m[2],m[3],m[4],m[5])); }
                worst = worst.max(err);
            }
            if best.as_ref().map_or(true, |b| worst < b.0) { best = Some((worst, sym.name.clone(), notes)); }
        }
        match best {
            Some((w, n, notes)) => { println!("character {j} ({n}): {} parts, worst error {w:.2} (1.0 = 0.01 in scale or 1 px)", want.len()); for x in notes { println!("   {x}"); } }
            None => println!("character {j}: no symbol with these layers"),
        }
    }
}
