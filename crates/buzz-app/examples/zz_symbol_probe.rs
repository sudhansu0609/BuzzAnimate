//! Diagnostic: dump what an imported symbol is made of, and how it measures.
//! cargo run --release -p buzz-app --example zz_symbol_probe -- input.fla "symbol name" ["symbol name" ...]

use kurbo::Shape as _;
fn describe(scene: &buzz_scene::Scene, object: &buzz_scene::Object, depth: usize, table: &std::collections::HashMap<buzz_scene::SymbolId, kurbo::Rect>) {
    let pad = "  ".repeat(depth);
    let t = object.transform.as_coeffs();
    let kind = match &object.kind {
        buzz_scene::ObjectKind::Shape(s) => {
            let paint = match s.fill.as_ref().map(|f| &f.paint) {
                Some(buzz_scene::Paint::Image(i)) => format!(
                    "image {}x{} px={} blob={} tile={} tf={:?}",
                    i.asset.width,
                    i.asset.height,
                    i.asset.pixels.len(),
                    i.asset.blob_id(),
                    i.tile,
                    i.transform.as_coeffs()
                ),
                Some(buzz_scene::Paint::Gradient(_)) => "gradient".to_string(),
                Some(p) => format!("solid {:?}", p.color()),
                None => "no fill".to_string(),
            };
            if std::env::var_os("PROBE_PATHS").is_some() {
                let mut sub = Vec::new();
                let mut cur = kurbo::BezPath::new();
                for el in s.path.elements() {
                    if matches!(el, kurbo::PathEl::MoveTo(_)) && !cur.elements().is_empty() {
                        sub.push(std::mem::take(&mut cur));
                    }
                    cur.push(*el);
                }
                if !cur.elements().is_empty() { sub.push(cur); }
                let areas: Vec<String> = sub.iter().map(|p| format!("{:.1}@{:?}", p.area(), p.bounding_box())).collect();
                println!("{pad}    subpaths={} total_area={:.1} perim={:.1} stroke_w={:?} areas={:?}", sub.len(), s.path.area(), s.path.perimeter(0.01), s.stroke.as_ref().map(|k| k.width), areas);
            }
            format!("shape path_bbox={:?} {paint} stroke={}", s.path.bounding_box(), s.stroke.is_some())
        }
        buzz_scene::ObjectKind::Group(children) => format!("group of {}", children.len()),
        buzz_scene::ObjectKind::Instance(i) => format!(
            "instance of {:?} ({}) first_frame={} loop={:?} color={:?}",
            scene.library().get(i.symbol).map(|s| s.name.clone()),
            i.symbol.0,
            i.first_frame,
            i.loop_mode,
            i.color
        ),
        buzz_scene::ObjectKind::Armature(_) => "armature".to_string(),
        buzz_scene::ObjectKind::Warp(_) => "warp".to_string(),
    };
    println!(
        "{pad}- {kind}\n{pad}    transform={:?} local_bounds={:?} resolved={:?} instance_bounds={:?}",
        t,
        object.local_bounds(),
        scene.resolved_bounds_with(object, table),
        scene.instance_bounds(object)
    );
    if let buzz_scene::ObjectKind::Group(children) = &object.kind {
        for c in children {
            describe(scene, c, depth + 1, table);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or_else(|| anyhow::anyhow!("missing input"))?;
    let imported = buzz_app::import::read(std::path::Path::new(&input)).map_err(anyhow::Error::msg)?;
    println!("summary: {}", imported.summary);
    println!("unsupported ({}):", imported.unsupported.len());
    for u in imported.unsupported.iter().take(40) {
        println!("  {u}");
    }
    let scene = imported.scene;
    let table = scene.symbol_bounds_table();
    if std::env::var_os("PROBE_STAGE").is_some() {
        println!("stage layers: {}", scene.stage_layers().len());
        for layer in scene.stage_layers().iter() {
            println!("stage layer {:?} kind={:?} visible={} keyframes={} length={}", layer.name, layer.kind, layer.visible, layer.frames.keyframes().len(), layer.frames.length());
            for k in layer.frames.keyframes().iter().take(3) {
                for object in k.objects.iter() {
                    if let buzz_scene::ObjectKind::Instance(i) = &object.kind {
                        println!("stage layer {:?} key {} -> {:?} id={} first_frame={} loop={:?}", layer.name, k.start, scene.library().get(i.symbol).map(|s| s.name.clone()), i.symbol.0, i.first_frame, i.loop_mode);
                    }
                }
            }
        }
    }
    if let Ok(spec) = std::env::var("PROBE_RESOLVE") {
        // PROBE_RESOLVE="symbol id,layer name,frame"
        let parts: Vec<&str> = spec.split(',').collect();
        let id = buzz_scene::SymbolId(parts[0].parse().unwrap());
        let frame: u32 = parts[2].parse().unwrap();
        let symbol = scene.library().get(id).unwrap();
        let layer = symbol.layers.iter().find(|l| l.name == parts[1]).unwrap();
        println!("layer {:?} length={} keyframe_at({})={:?}", layer.name, layer.frames.length(), frame, layer.frames.keyframe_at(frame).map(|k| k.start));
        for object in symbol.layers.resolved_at_rigged(layer.id, frame).iter() {
            if let buzz_scene::ObjectKind::Instance(i) = &object.kind {
                println!("  resolved_at_rigged -> {:?} first_frame={} loop={:?} sync={}", scene.library().get(i.symbol).map(|s| s.name.clone()), i.first_frame, i.loop_mode, i.sync_to_root);
            }
        }
        for object in layer.frames.resolved_at(frame).iter() {
            if let buzz_scene::ObjectKind::Instance(i) = &object.kind {
                println!("  resolved_at -> first_frame={} loop={:?}", i.first_frame, i.loop_mode);
            }
        }
    }
    for name in args {
        let matches: Vec<_> = scene.library().iter().filter(|s| s.name == name || s.name.ends_with(&format!("/{name}"))).collect();
        println!("\n=== {name}: {} symbol(s)", matches.len());
        for symbol in matches {
            println!("symbol {:?} id={} length={} table_bounds={:?}", symbol.name, symbol.id.0, symbol.length(), table.get(&symbol.id));
            for layer in symbol.layers.iter() {
                println!(" layer {:?} kind={:?} visible={} keyframes={}", layer.name, layer.kind, layer.visible, layer.frames.keyframes().len());
                for k in layer.frames.keyframes() {
                    println!("  keyframe start={} objects={}", k.start, k.objects.len());
                    for object in k.objects.iter() {
                        describe(&scene, object, 2, &table);
                    }
                }
            }
        }
    }
    Ok(())
}
