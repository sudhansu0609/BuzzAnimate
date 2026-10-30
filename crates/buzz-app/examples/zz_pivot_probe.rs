//! scratch probe — delete after use. Free Transform rotation on each piece of
//! a rigged character: does the transformation point stay put?
//!
//! zz_pivot_probe <doc.buzz> <symbol>
use buzz_app::editor::Editor;
use buzz_geom::{Point, Size};
use buzz_ui::ToolId;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    unsafe {
        std::env::set_var(
            "BUZZANIMATE_WORKSPACE",
            std::env::temp_dir().join(format!("buzz-pivot-probe-{}.json", std::process::id())),
        )
    };
    let mut e = Editor::default();
    e.workspace = buzz_ui::Workspace::animate();
    e.doc = buzz_doc::Document::open(&a[0]).expect("open");
    let sym = e.scene().library().iter().find(|s| s.name == a[1]).map(|s| s.id).expect("symbol");
    e.doc.edit_view(|scene| {
        scene.enter_symbol(sym);
    });
    e.camera.viewport = Size::new(1600.0, 1000.0);
    e.camera.set_zoom_percent(100.0);

    let find = |e: &Editor, name: &str| -> (buzz_scene::LayerId, buzz_scene::ObjectId) {
        let l = e.scene().layers().iter().find(|l| l.name == name).expect("layer");
        (l.id, l.objects_at(0)[0].id)
    };
    let joint = |e: &Editor, layer: buzz_scene::LayerId, id: buzz_scene::ObjectId| -> Point {
        let scene = e.scene();
        let (_, o) = scene.find_object(id).expect("o");
        let follows = scene.layers().inherited_transform(layer, 0);
        follows * (o.transform * o.pivot.expect("stored pivot"))
    };
    let (arm_l, arm) = find(&e, "rightArm");
    let (wrist_l, wrist) = find(&e, "rightWrist");
    let (palm_l, palm) = find(&e, "rightPalm");
    let before = (joint(&e, wrist_l, wrist), joint(&e, palm_l, palm));
    e.set_tool(ToolId::Selection);
    e.selection.select_one(arm);
    e.set_tool(ToolId::FreeTransform);
    let b = e.selection_bounds_drawn().expect("bounds");
    e.camera.center = b.center();
    let cam = e.camera.clone();
    let from = Point::new(b.x1 + 14.0, b.y1 + 14.0);
    let to = Point::new(b.x1 - 14.0, b.y1 + 42.0);
    e.pointer_down(cam.doc_to_screen(from), Default::default());
    e.pointer_move(cam.doc_to_screen(to), Default::default());
    e.pointer_up(cam.doc_to_screen(to));
    println!("arm turned: xf {:?}", e.scene().find_object(arm).unwrap().1.transform.as_coeffs().map(|v| (v * 1000.0).round() / 1000.0));
    for (name, l, id, was) in [("rightWrist", wrist_l, wrist, before.0), ("rightPalm", palm_l, palm, before.1)] {
        e.set_tool(ToolId::Selection);
        e.selection.select_one(id);
        e.set_tool(ToolId::FreeTransform);
        let shown = e.pivot().expect("pivot");
        let real = joint(&e, l, id);
        let drawn = e.selection_bounds_drawn().expect("b");
        println!("{name}: joint before {:?} joint now (follows*T*stored) {:?}  white circle shown at {:?}  -> off by {:.1}; box {:?}", (was.x.round(), was.y.round()), (real.x.round(), real.y.round()), (shown.x.round(), shown.y.round()), (shown - real).hypot(), drawn);
    }
    // Now turn the wrist: it must turn about its joint, which stays put.
    for (cx, cy) in [(1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        e.set_tool(ToolId::Selection);
        e.selection.select_one(wrist);
        e.set_tool(ToolId::FreeTransform);
        let j0 = joint(&e, wrist_l, wrist);
        let c0 = e.pivot().unwrap();
        let b = e.selection_bounds_drawn().expect("bounds");
        e.camera.center = b.center();
        let cam = e.camera.clone();
        let tb = e.selection_box().expect("box");
        let l = tb.local;
        let lc = Point::new(if cx > 0.0 { l.x1 } else { l.x0 }, if cy > 0.0 { l.y1 } else { l.y0 });
        let corner = tb.to_doc * lc;
        let out = (corner - tb.to_doc * l.center()).normalize();
        let from = corner + out * 14.0;
        let side = buzz_geom::Vec2::new(-out.y, out.x);
        let to = from + side * 40.0;
        let before = e.scene().find_object(wrist).unwrap().1.transform;
        e.pointer_down(cam.doc_to_screen(from), Default::default());
        e.pointer_move(cam.doc_to_screen(to), Default::default());
        e.pointer_up(cam.doc_to_screen(to));
        let after = e.scene().find_object(wrist).unwrap().1.transform;
        let c = after.as_coeffs();
        let rot = c[1].abs() > 1e-3 && ((c[0] * c[3] - c[1] * c[2]) - 1.0).abs() < 1e-3;
        if before.as_coeffs() != c {
            let j1 = joint(&e, wrist_l, wrist);
            println!("corner {cx},{cy}: rotation {rot}  joint {:?} -> {:?} (moved {:.3}); circle before {:?}", (j0.x.round(), j0.y.round()), (j1.x.round(), j1.y.round()), (j1 - j0).hypot(), (c0.x.round(), c0.y.round()));
            if rot { break; }
        } else {
            let sel: Vec<String> = e.selection.ids().iter().filter_map(|i| e.scene().find_object(*i).and_then(|(l, _)| e.scene().layers().get(l).map(|l| l.name.clone()))).collect();
            println!("corner {cx},{cy}: no change; selected now {sel:?}; box {:?} from {:?}", b, from);
        }
    }
    return;
    let pieces: Vec<(String, buzz_scene::ObjectId)> = e
        .scene()
        .layers()
        .iter()
        .filter_map(|l| l.objects_at(0).first().map(|o| (l.name.clone(), o.id)))
        .collect();
    for (name, id) in pieces {
        e.set_tool(ToolId::Selection);
        e.selection.select_one(id);
        e.set_tool(ToolId::FreeTransform);
        let Some(bounds) = e.selection_bounds_drawn() else { continue };
        e.camera.center = bounds.center();
        let p0 = e.pivot();
        let (kind, stored, xf) = {
            let (_, o) = e.scene().find_object(id).expect("o");
            let k = match &o.kind { buzz_scene::ObjectKind::Shape(_) => "shape".to_string(), buzz_scene::ObjectKind::Group(c) => format!("group({})", c.len()), buzz_scene::ObjectKind::Instance(_) => "instance".into(), _ => "other".into() };
            (k, o.pivot, o.transform.as_coeffs().map(|v| (v * 100.0).round() / 100.0))
        };
        let mut line = format!("{name:12} {kind} stored {stored:?} xf {xf:?} bounds-centre {:?} pivot0 {:?}", bounds.center(), p0.map(|p| (p.x.round(), p.y.round())));
        for turn in 0..2 {
            let bounds = e.selection_bounds_drawn().expect("bounds");
            let cam = e.camera.clone();
            let off: f64 = std::env::var("OFF").ok().and_then(|v| v.parse().ok()).unwrap_or(16.0);
            let from = Point::new(bounds.x1 + off, bounds.y1 + off);
            let to = Point::new(bounds.x1 - off, bounds.y1 + off * 3.0);
            e.pointer_down(cam.doc_to_screen(from), Default::default());
            e.pointer_move(cam.doc_to_screen(to), Default::default());
            e.pointer_up(cam.doc_to_screen(to));
            let p = e.pivot();
            let shift = match (p0, p) {
                (Some(a), Some(b)) => (b - a).hypot(),
                _ => f64::NAN,
            };
            line += &format!("  turn{turn}: pivot {:?} shift {shift:.2}", p.map(|p| (p.x.round(), p.y.round())));
            if let Some((_, o)) = e.scene().find_object(id) { let c = o.transform.as_coeffs().map(|v| (v * 1000.0).round() / 1000.0); let p = o.pivot.unwrap_or_default(); line += &format!(" xf {c:?} T*stored {:?}", o.transform * p); }
            if e.selection.ids().is_empty() { line += " (selection lost)"; break; }
        }
        println!("{line}");
    }
}
