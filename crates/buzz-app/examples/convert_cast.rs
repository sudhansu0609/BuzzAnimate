//! **LR3: the 12 Ep1 Cast bone rigs as layer rigs**, saved as new assets.
//!
//! `cargo run --release -p buzz-app --example convert_cast`
//!
//! For each Ep1 Cast rig in the user's asset library (`Cast/<name>`), load it,
//! take it apart with the Rig panel's **Convert to layer rig**
//! (`rigging::bones_to_layers`), check `layer_rig::rig_of` reads it back as a
//! layer rig and that every symbol it had (eye variants, mouth charts) is
//! still there, and save it as `Cast/Layers/<name>`. The originals are only
//! read. A rig with skinned parts refuses to convert and is skipped.
//!
//! `... --example convert_cast -- --sheets <out-dir>`: one sheet per rig,
//! `<out-dir>/lr3-<name>.png` — the original bone rig above the **saved**
//! `Cast/Layers` asset (loaded back from disk), at rest and in Wave, Talk,
//! mid-stride Walk and Point, each written with `perform::apply`.
//!
//! Not a test: whether the layer rig looks like the bone rig is for a person
//! looking at the sheets.

use std::collections::BTreeSet;

use buzz_act::layer_rig::{self, CharacterRig};
use buzz_act::perform::{self, Action, Performance};
use buzz_doc::assets::{Asset, AssetLibrary};
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;
use buzz_scene::{Object, ObjectId, ObjectKind, Scene, SymbolId};
use image::{Rgba, RgbaImage};

/// The 12 Ep1 Cast rigs.
const NAMES: [&str; 12] = [
    "Raju",
    "Raju Back",
    "Munna",
    "Munna Back",
    "Guddu",
    "Guddu 3-4",
    "Guddu Back",
    "Lalten",
    "Lalten Back",
    "Woman1",
    "Woman1 3-4",
    "Woman1 Back",
];
const SOURCE: &str = "Cast";
const TARGET: &str = "Cast/Layers";

const CELL_W: u32 = 400;
const CELL_H: u32 = 480;
const LABEL_H: u32 = 28;
const FRAMES: u32 = 36;
const BACKGROUND: [u8; 3] = [0xF2, 0xF0, 0xEA];

/// Rest, Wave, Talk, mid-stride Walk (on the spot) and Point.
const CELLS: [(&str, Option<Action>, u32); 5] = [
    ("REST", None, 0),
    ("WAVE", Some(Action::Wave), 15),
    ("TALK", Some(Action::Talk), 4),
    ("WALK", Some(Action::Walk), 9),
    ("POINT", Some(Action::Point), 18),
];

fn asset<'a>(library: &'a AssetLibrary, folder: &str, name: &str) -> Option<&'a Asset> {
    library
        .in_folder(folder)
        .into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(name))
}

/// The first visible armature on frame 0, as the episode builder finds a
/// Cast rig's body.
fn armature_of(scene: &Scene) -> Option<ObjectId> {
    scene.layers().iter().find_map(|layer| {
        layer
            .objects_at(0)
            .iter()
            .find(|o| o.visible && matches!(o.kind, ObjectKind::Armature(_)))
            .map(|o| o.id)
    })
}

/// The root part of the first layer rig in the scene.
fn layer_body_of(scene: &Scene) -> Option<ObjectId> {
    scene
        .layers()
        .iter()
        .find_map(|l| layer_rig::rig_of_layer(scene, l.id))
        .map(|view| view.body)
}

/// Every symbol an object uses, through groups and armature parts.
fn used_symbols(object: &Object, into: &mut BTreeSet<SymbolId>) {
    match &object.kind {
        ObjectKind::Instance(instance) => {
            into.insert(instance.symbol);
        }
        ObjectKind::Group(children) => {
            for child in children {
                used_symbols(child, into);
            }
        }
        ObjectKind::Armature(rig) => {
            for part in &rig.parts {
                used_symbols(&part.artwork, into);
            }
        }
        _ => {}
    }
}

/// `(library symbol names, symbols used on the main timeline)`.
fn symbols(scene: &Scene) -> (BTreeSet<String>, BTreeSet<SymbolId>) {
    let names = scene.library().iter().map(|s| s.name.clone()).collect();
    let mut used = BTreeSet::new();
    for layer in scene.layers().iter() {
        for object in layer.all_objects() {
            used_symbols(object, &mut used);
        }
    }
    (names, used)
}

fn convert() -> anyhow::Result<()> {
    let mut library = AssetLibrary::user();
    library
        .create_folder(TARGET)
        .map_err(|e| anyhow::anyhow!("create {TARGET}: {e:?}"))?;
    let mut skipped = Vec::new();
    for name in NAMES {
        let Some(original) = asset(&library, SOURCE, name).cloned() else {
            println!("{name}: SKIPPED - no asset {SOURCE}/{name}");
            skipped.push(name);
            continue;
        };
        let mut scene = library
            .load(&original)
            .map_err(|e| anyhow::anyhow!("{name}: {e:?}"))?;
        let Some(body) = armature_of(&scene) else {
            println!("{name}: SKIPPED - no armature on frame 0");
            skipped.push(name);
            continue;
        };
        let mut warnings: Vec<String> = Vec::new();
        let (bones, parts_before, poses) = match scene.find_object(body).map(|(_, o)| &o.kind) {
            Some(ObjectKind::Armature(rig)) => (rig.armature.len(), rig.parts.len(), rig.poses.len()),
            _ => (0, 0, 0),
        };
        if poses > 0 {
            warnings.push(format!("{poses} saved armature poses are not carried to the layer rig"));
        }
        let (names_before, used_before) = symbols(&scene);
        let layers_before = scene.layers().len();

        let Some(rig) = buzz_app::rigging::bones_to_layers(&mut scene, 0, body) else {
            println!("{name}: SKIPPED - refuses to convert (skinned artwork)");
            skipped.push(name);
            continue;
        };

        let root_part = scene
            .layers()
            .get(rig.root)
            .and_then(|l| l.objects_at(0).first())
            .map(|o| o.id);
        let view = match root_part.and_then(|p| layer_rig::rig_of(&scene, p)) {
            Some(CharacterRig::Layers(view)) => Some(view),
            other => {
                warnings.push(format!("rig_of does not read it as a layer rig: {other:?}"));
                None
            }
        };
        let links = rig
            .layers
            .iter()
            .filter(|(_, l)| scene.layers().get(*l).is_some_and(|l| l.follows.is_some()))
            .count();
        if let Some(view) = &view {
            if view.parts.len() != rig.layers.len() {
                warnings.push(format!(
                    "rig_of sees {} part layers of {} made ({} pattern)",
                    view.parts.len(),
                    rig.layers.len(),
                    view.pattern.name
                ));
            }
        }
        if rig.layers.len() != parts_before {
            warnings.push(format!("{parts_before} armature parts became {} layers", rig.layers.len()));
        }
        if armature_of(&scene).is_some() {
            warnings.push("another armature is still on frame 0".into());
        }
        let (names_after, used_after) = symbols(&scene);
        let symbols_kept = names_before == names_after && used_before == used_after;
        if !symbols_kept {
            warnings.push(format!(
                "symbols changed: library {} -> {}, used {} -> {}",
                names_before.len(),
                names_after.len(),
                used_before.len(),
                used_after.len()
            ));
        }
        if buzz_doc::assets::rig_kind_of(&scene) != Some(buzz_rig::RigKind::Layers) {
            warnings.push(format!(
                "the asset library will list it as {:?}",
                buzz_doc::assets::rig_kind_of(&scene)
            ));
        }

        let saved = library
            .save(name, TARGET, &scene)
            .map_err(|e| anyhow::anyhow!("{name}: save: {e:?}"))?;
        // Read it back as the episode will.
        let back = library
            .load(&saved)
            .map_err(|e| anyhow::anyhow!("{name}: reload: {e:?}"))?;
        match layer_body_of(&back).and_then(|b| layer_rig::rig_of(&back, b)) {
            Some(CharacterRig::Layers(v)) if v.parts.len() == rig.layers.len() => {}
            other => warnings.push(format!("reloaded asset reads as {:?}", other.map(|r| r.body()))),
        }

        println!(
            "{name}: bones {bones}, parts {}, layers {} (was {layers_before}), followed links {links}, \
             pattern {}, symbols kept {symbols_kept} ({} in library, {} used), saved {}, warnings {:?}",
            rig.layers.len(),
            scene.layers().len(),
            view.as_ref().map_or("-".to_string(), |v| v.pattern.name.clone()),
            names_after.len(),
            used_after.len(),
            saved.path.display(),
            warnings
        );
    }
    println!("skipped: {skipped:?}");
    Ok(())
}

/// A 5x7 bitmap font: each glyph is seven rows of five bits, high bit left.
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        _ => [0; 7],
    }
}

/// `text` in dark grey at `(x, y)`, each font pixel `scale` image pixels.
fn label(img: &mut RgbaImage, text: &str, x: u32, y: u32, scale: u32) {
    let ink = Rgba([0x30, 0x30, 0x38, 255]);
    for (k, c) in text.to_uppercase().chars().enumerate() {
        let rows = glyph(c);
        let gx = x + k as u32 * 6 * scale;
        for (ry, row) in rows.iter().enumerate() {
            for rx in 0..5u32 {
                if row & (0x10 >> rx) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (gx + rx * scale + dx, y + ry as u32 * scale + dy);
                        if px < img.width() && py < img.height() {
                            img.put_pixel(px, py, ink);
                        }
                    }
                }
            }
        }
    }
}

/// Render `frame` of `scene` over `region` into the sheet at `(x0, y0)`, with
/// `title` over it.
fn paste_cell(
    exporter: &mut Exporter,
    sheet: &mut RgbaImage,
    scene: &Scene,
    frame: u32,
    region: kurbo::Rect,
    (x0, y0): (u32, u32),
    title: &str,
) -> anyhow::Result<()> {
    let mut settings = ExportSettings::for_stage(scene);
    settings.width = CELL_W;
    settings.height = CELL_H - LABEL_H;
    settings.region = Some(region);
    let picture = exporter.render(scene, frame, &settings)?;
    let cell = RgbaImage::from_raw(picture.width, picture.height, picture.pixels)
        .ok_or_else(|| anyhow::anyhow!("the rendered cell has the wrong buffer size"))?;
    for (x, y, p) in cell.enumerate_pixels() {
        let a = p[3] as u32;
        let mix = |c: u8, b: u8| ((c as u32 * a + b as u32 * (255 - a)) / 255) as u8;
        sheet.put_pixel(
            x0 + x,
            y0 + LABEL_H + y,
            Rgba([
                mix(p[0], BACKGROUND[0]),
                mix(p[1], BACKGROUND[1]),
                mix(p[2], BACKGROUND[2]),
                255,
            ]),
        );
    }
    label(sheet, title, x0 + 8, y0 + 6, 2);
    for x in x0..x0 + CELL_W {
        sheet.put_pixel(x, y0, Rgba([0xC8, 0xC4, 0xBC, 255]));
    }
    for y in y0..y0 + CELL_H {
        sheet.put_pixel(x0, y, Rgba([0xC8, 0xC4, 0xBC, 255]));
    }
    Ok(())
}

fn sheets(out: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(out)?;
    let library = AssetLibrary::user();
    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    for name in NAMES {
        let (Some(original), Some(converted)) = (asset(&library, SOURCE, name), asset(&library, TARGET, name))
        else {
            println!("{name}: no {SOURCE} or {TARGET} asset, no sheet");
            continue;
        };
        let mut base = library.load(original).map_err(|e| anyhow::anyhow!("{name}: {e:?}"))?;
        let mut layered = library.load(converted).map_err(|e| anyhow::anyhow!("{name}: {e:?}"))?;
        let body = armature_of(&base).ok_or_else(|| anyhow::anyhow!("{name}: no armature"))?;
        let layer_body = layer_body_of(&layered).ok_or_else(|| anyhow::anyhow!("{name}: no layer rig"))?;
        for scene in [&mut base, &mut layered] {
            scene.stage_mut().background =
                peniko::Color::from_rgb8(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2]);
        }

        let (bounds, cx) = {
            let (_, object) = base.find_object(body).expect("the body");
            let cx = match &object.kind {
                ObjectKind::Armature(rig) => (object.transform * rig.armature.head(0)).x,
                _ => base.resolved_bounds(object).center().x,
            };
            (base.resolved_bounds(object), cx)
        };
        let aspect = CELL_W as f64 / (CELL_H - LABEL_H) as f64;
        let mut height = bounds.height() * 1.35;
        let mut width = height * aspect;
        if width < bounds.width() * 1.7 {
            width = bounds.width() * 1.7;
            height = width / aspect;
        }
        let top = bounds.y1 + bounds.height() * 0.06 - height;
        let region = kurbo::Rect::new(cx - width / 2.0, top, cx + width / 2.0, top + height);

        let mut sheet = RgbaImage::from_pixel(
            CELLS.len() as u32 * CELL_W,
            2 * CELL_H + 40,
            Rgba([BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]),
        );
        label(&mut sheet, &format!("{name} - bones above, saved layer asset below"), 12, 8, 4);
        for (k, (title, action, frame)) in CELLS.iter().enumerate() {
            for (row, (source, who, kind)) in
                [(&base, body, "BONES"), (&layered, layer_body, "LAYERS")].into_iter().enumerate()
            {
                let mut scene = source.clone();
                if let Some(action) = action {
                    let mut performance = Performance::new(*action, 0..FRAMES);
                    performance.distance = 0.0;
                    perform::apply(&mut scene, who, &performance)
                        .map_err(|e| anyhow::anyhow!("{name} {title} {kind}: {e}"))?;
                }
                let at = (k as u32 * CELL_W, 40 + row as u32 * CELL_H);
                paste_cell(&mut exporter, &mut sheet, &scene, *frame, region, at, &format!("{kind} {title} F{frame}"))?;
            }
        }
        let file = out.join(format!("lr3-{name}.png"));
        sheet.save(&file)?;
        println!("wrote {}", file.display());
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().position(|a| a == "--sheets") {
        Some(i) => sheets(std::path::Path::new(args.get(i + 1).map_or(".", |s| s.as_str()))),
        None => convert(),
    }
}
