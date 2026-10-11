//! zz_tween_probe file.fla layer_name — for every symbol with that layer,
//! print its keyframes' tween state and the resolved transform per frame.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let imported = buzz_app::import::read(std::path::Path::new(&a[0])).unwrap();
    for sym in imported.scene.library().iter() {
        let Some(layer) = sym.layers.iter().find(|l| l.name == a[1]) else { continue };
        if !sym.layers.iter().any(|l| l.name == "kudal") { continue; }
        println!("== {}", sym.name);
        for k in layer.frames.keyframes() {
            println!("  key {} tween {:?} ease_curve {:?}", k.start, k.tween, k.ease_curve);
            if let Some(set) = &k.ease_curve {
                if let Some(all) = &set.all {
                    println!("    all(0.5) = {}", buzz_scene::eval_bezier_ease(all, 0.5));
                }
            }
        }
        for f in [0u32, 2, 5, 8, 11, 12, 16, 20, 23, 26] {
            let r = sym.layers.resolved_at_rigged(layer.id, f);
            let t = r.iter().next().map(|o| o.transform.as_coeffs());
            println!("  f{f:>2} {:?}", t.map(|c| format!("{:.3} {:.3} {:.3} {:.3} {:.1} {:.1}", c[0], c[1], c[2], c[3], c[4], c[5])));
        }
    }
}
