//! **X1 proof: Extend Part Under Neighbours, headless.**
//!
//! Builds a stage with a striped arm symbol (base colour, three stripes along
//! the arm, an outline on its top, bottom and far end, none across the cut)
//! under a square torso, the arm's transformation point on the joint inside
//! the torso. Runs `Command::ExtendPart`, then writes PNGs to the folder given:
//!
//! - `rest_before.png` / `rest_after.png`: torso visible, at rest.
//! - `hidden_before.png` / `hidden_after.png`: torso hidden, so the extension
//!   shows under it.
//! - `rotated_before.png` / `rotated_after.png`: the arm turned 60° about the
//!   joint, torso visible.
//! - `hidden_undone.png`: after Undo, torso hidden.
//!
//! Prints the status, the shapes on the "Extension" layer, the rest diff
//! (pixels differing by more than 24 in any channel), how many extension
//! pixels lie outside the torso's footprint, and whether Undo removed the layer.
//!
//! ```text
//! cargo run -p buzz-app --release --example extend_part_check -- <out-dir>
//! ```

use std::path::PathBuf;

use buzz_app::editor::Editor;
use buzz_doc::Document;
use buzz_geom::{Affine, BezPath, Point, Rect, Shape as _};
use buzz_scene::{LayerKind, ObjectId, Scene, ShapeData, SymbolId, SymbolKind};
use buzz_ui::Command;
use peniko::Color;

const SCALE: f64 = 2.0;

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    Rect::new(x0, y0, x1, y1).to_path(0.1)
}

fn render(exporter: &mut buzz_export::Exporter, scene: &Scene) -> image::RgbaImage {
    let settings = buzz_export::ExportSettings::scaled(scene, SCALE);
    let frame = exporter.render(scene, 0, &settings).expect("render");
    image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels).expect("rgba")
}

fn with(scene: &Scene, id: ObjectId, f: impl FnOnce(&mut buzz_scene::Object)) -> Scene {
    let mut scene = scene.clone();
    scene.update_object(id, f);
    scene
}

fn extension_shapes(scene: &Scene, symbol: SymbolId) -> Option<usize> {
    let s = scene.library().get(symbol)?;
    let layer = s.layers.iter().find(|l| l.name == "Extension")?;
    Some(layer.all_objects().count())
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("usage: extend_part_check <out-dir>"));
    std::fs::create_dir_all(&out).expect("out dir");

    let mut scene = Scene::default();
    let arm_layer = scene.add_layer("Arm", LayerKind::Normal);
    let torso_layer = scene.add_layer("Torso", LayerKind::Normal);

    // The arm, in its own space: cut at x = 50 (inside the torso), running to
    // x = 240. The joint is the origin.
    let arm = scene.add_symbol("Arm", SymbolKind::Graphic, None);
    let inner = scene.library().get(arm).and_then(|s| s.layers.iter().next().map(|l| l.id)).unwrap();
    scene.enter_symbol(arm);
    scene.add_shape(inner, ShapeData::filled(rect(50.0, -30.0, 240.0, 30.0), Color::from_rgb8(240, 200, 150)));
    for (y0, y1, c) in [
        (-22.0, -14.0, Color::from_rgb8(200, 40, 40)),
        (-4.0, 4.0, Color::from_rgb8(40, 90, 200)),
        (14.0, 22.0, Color::from_rgb8(40, 160, 60)),
    ] {
        scene.add_shape(inner, ShapeData::filled(rect(50.0, y0, 240.0, y1), c));
    }
    let mut outline = BezPath::new();
    outline.move_to((50.0, -30.0));
    outline.line_to((240.0, -30.0));
    outline.line_to((240.0, 30.0));
    outline.line_to((50.0, 30.0));
    scene.add_shape(inner, ShapeData::stroked(outline, Color::from_rgb8(30, 20, 20), 4.0));
    scene.exit_symbol();

    let joint = Point::new(250.0, 200.0);
    let arm_id = scene
        .add_instance_at(arm_layer, 0, arm, Affine::translate(joint.to_vec2()))
        .expect("arm instance");
    scene.update_object(arm_id, |o| o.pivot = Some(Point::ORIGIN));
    // Its right edge 5 units past the cut, and only 10 units taller than the
    // arm on each side, so turning the arm uncovers the extension.
    let torso = (joint.x - 120.0, joint.y - 40.0, joint.x + 55.0, joint.y + 40.0);
    let torso_id = scene
        .add_shape(torso_layer, ShapeData::filled(rect(torso.0, torso.1, torso.2, torso.3), Color::from_rgb8(90, 110, 140)))
        .expect("torso");

    let mut editor = Editor::new(Document::new(scene));
    editor.selection.toggle(arm_id);
    let before = editor.scene().clone();
    editor.run(Command::ExtendPart);
    let after = editor.scene().clone();
    println!("status: {}", editor.status.clone().unwrap_or_default());
    println!("extension shapes: {:?}", extension_shapes(&after, arm));

    let mut exporter = buzz_export::Exporter::new(&buzz_render::GpuPreference::default()).expect("exporter");
    let rotate = |o: &mut buzz_scene::Object| {
        o.transform = Affine::translate(joint.to_vec2())
            * Affine::rotate(60f64.to_radians())
            * Affine::translate(-joint.to_vec2())
            * o.transform;
    };
    let hide = |o: &mut buzz_scene::Object| o.visible = false;
    let rest_before = render(&mut exporter, &before);
    let rest_after = render(&mut exporter, &after);
    let hidden_before = render(&mut exporter, &with(&before, torso_id, hide));
    let hidden_after = render(&mut exporter, &with(&after, torso_id, hide));
    let rotated_before = render(&mut exporter, &with(&before, arm_id, rotate));
    let rotated_after = render(&mut exporter, &with(&after, arm_id, rotate));
    for (name, img) in [
        ("rest_before", &rest_before),
        ("rest_after", &rest_after),
        ("hidden_before", &hidden_before),
        ("hidden_after", &hidden_after),
        ("rotated_before", &rotated_before),
        ("rotated_after", &rotated_after),
    ] {
        img.save(out.join(format!("{name}.png"))).expect("save");
    }

    // Rest with the torso visible must not change.
    let differ = |a: &image::RgbaImage, b: &image::RgbaImage| {
        a.pixels()
            .zip(b.pixels())
            .filter(|(p, q)| p.0.iter().zip(q.0.iter()).any(|(x, y)| x.abs_diff(*y) > 24))
            .count()
    };
    println!("rest diff (>24): {} px", differ(&rest_before, &rest_after));

    // What the extension added with the torso hidden, and how much of it lies
    // outside the torso's footprint (1 px anti-alias allowed).
    let (mut added, mut outside) = (0usize, 0usize);
    for (x, y, p) in hidden_after.enumerate_pixels() {
        let q = hidden_before.get_pixel(x, y);
        if !p.0.iter().zip(q.0.iter()).any(|(a, b)| a.abs_diff(*b) > 24) {
            continue;
        }
        added += 1;
        let (dx, dy) = ((x as f64 + 0.5) / SCALE, (y as f64 + 0.5) / SCALE);
        let m = 1.0 / SCALE;
        if dx < torso.0 - m || dx > torso.2 + m || dy < torso.1 - m || dy > torso.3 + m {
            outside += 1;
        }
    }
    println!("extension px (torso hidden): {added}, outside the torso: {outside}");
    println!("rotated diff (>24): {} px", differ(&rotated_before, &rotated_after));

    editor.run(Command::Undo);
    let undone = editor.scene().clone();
    println!("after undo, extension layer: {:?}", extension_shapes(&undone, arm));
    let hidden_undone = render(&mut exporter, &with(&undone, torso_id, hide));
    hidden_undone.save(out.join("hidden_undone.png")).expect("save");
    println!("undo vs before (torso hidden, >24): {} px", differ(&hidden_before, &hidden_undone));
}
