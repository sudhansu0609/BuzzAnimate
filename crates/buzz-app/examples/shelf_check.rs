//! Temporary: run shelf commands on a small character-like scene and print what changed.
use buzz_scene::{LayerKind, Object, Scene};
fn main() {
    let config = buzz_script::default_config_dir().expect("a shelf");
    println!("shelf: {}", config.display());
    for name in ["eyebrowsUp", "eyeBrowDown", "rotateEyeBrowUp"] {
        let mut scene = Scene::default();
        for layer_name in ["eyeBall", "leftEyeBrow", "rightEyeBrow", "head"] {
            let layer = scene.add_layer(layer_name, LayerKind::Normal);
            scene.update_layer(layer, |l| { while l.frames.length() < 60 { l.frames.insert_frame(l.frames.length()); } });
            let symbol = buzz_act::lipsync::placeholder_mouth(&mut scene, &format!("{layer_name}_sym"));
            let mut group = Object::instance_of(scene.next_object_id(), symbol);
            group.transform = buzz_geom::Affine::translate((100.0, 200.0));
            let _ = scene.add_object(layer, group);
        }
        let source = std::fs::read_to_string(config.join("Commands").join(format!("{name}.jsfl"))).unwrap();
        let ctx = buzz_script::ScriptContext { config_dir: Some(config.clone()), current_frame: 10, ..Default::default() };
        let out = buzz_script::run(&mut scene, ctx, &source, &buzz_script::Limits::default());
        println!("\n== {name}: error={:?} trace={:?} alerts={:?}", out.error, out.trace.iter().take(3).collect::<Vec<_>>(), out.alerts);
        for l in scene.layers().iter() {
            let keys: Vec<String> = l.frames.keyframes().iter().map(|k| {
                let x = k.objects.first().map(|o| { let t = o.transform.as_coeffs(); format!("({:.0},{:.0})", t[4], t[5]) }).unwrap_or_default();
                format!("{}{}{}", k.start, x, if k.tween.kind != buzz_scene::TweenKind::None { "~" } else { "" })
            }).collect();
            println!("  {:<10} {}", l.name, keys.join(" "));
        }
    }
}
