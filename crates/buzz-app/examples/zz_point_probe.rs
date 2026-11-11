//! scratch probe: which shapes of the first layer contain a stage point.
//! Usage: zz_point_probe <file.buzz> x y
use buzz_geom::Shape as _;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let doc = buzz_doc::Document::open(&a[0]).unwrap();
    let e = buzz_app::editor::Editor::new(doc);
    let c = e.scene().content_bounds().unwrap();
    let p = buzz_geom::Point::new(c.x0 + a[1].parse::<f64>().unwrap(), c.y0 + a[2].parse::<f64>().unwrap());
    for l in e.scene().layers().iter() {
        for o in l.objects_at(0) {
            if let buzz_scene::ObjectKind::Shape(s) = &o.kind {
                let path = o.transform * s.path.clone();
                if path.contains(p) {
                    let b = path.bounding_box();
                    let col = s.fill.as_ref().map(|f| match &f.paint { buzz_scene::Paint::Solid(c) => format!("{:?}", c.to_rgba8()), _ => "other".into() });
                    println!("id {} fill {:?} stroke {} bbox {:.0}x{:.0} area {:.0} segs {}", o.id.0, col, s.stroke.is_some(), b.width(), b.height(), path.area().abs(), buzz_app::rigging::segment_count(&path));
                }
            }
        }
    }
}
