//! Render the one-shot gestures on real Cast rigs, for looking at.
//!
//! `cargo run --release -p buzz-app --example gesture_sheet -- <out-dir> [rig.buzz | puppet ...]`
//!
//! With `--layers`, one sheet per Cast rig instead (`layers-<rig>.png`): its
//! bone version above its layer version (Bones → Layers on a copy in memory),
//! at rest and in Wave, Shrug, Point, Talk and mid-stride Walk — the layer
//! rig must look the same as the bone rig it came from.
//!
//! With `--walk`, one sheet per Cast rig (`walk-<rig>.png`): eight evenly
//! spaced frames of one Walk cycle and one Run cycle, bone rig above layer
//! rig, with the view the rig was read as (`perform::rig_view`).
//!
//! Not a test: whether a shrug reads as a shrug is something only a person
//! looking at it can judge. For each rig (default: the four Ep1 front-view
//! Cast rigs and the procedural puppet the director stages, `puppet`) every
//! gesture is written onto the rig with `perform::apply` over a 36-frame
//! range, exactly as the episode builder writes one, and the frame at its
//! peak is rendered — and for Wave, Nod and Shake Head a second frame at the
//! other phase. One contact sheet per rig, `<out-dir>/gestures-<rig>.png`,
//! with a rest-pose cell first.
//!
//! The two kinds of rig hang their arms opposite ways round (see
//! `perform::arms_mirrored`), which is why both are on the sheet: a pose
//! that is right on one and wrong on the other shows up side by side.
//!
//! The rig's bones (name, parent, rest and world angles, limits) are printed
//! too, which is what the sign conventions in `pose_at` have to agree with.

use buzz_act::perform::{self, Action, Performance};
use buzz_export::{ExportSettings, Exporter};
use buzz_render::GpuPreference;
use buzz_scene::{ObjectId, ObjectKind, Scene};
use image::{Rgba, RgbaImage};

const CELL_W: u32 = 400;
const CELL_H: u32 = 480;
const LABEL_H: u32 = 28;
const FRAMES: u32 = 36;
const BACKGROUND: [u8; 3] = [0xF2, 0xF0, 0xEA];

/// The four Ep1 front views, and the procedural puppet.
const DEFAULT_RIGS: [&str; 5] = [
    r"B:\youtubeProjects\Originals\series\ep1\buzz\rigs\Raju.buzz",
    r"B:\youtubeProjects\Originals\series\ep1\buzz\rigs\Munna.buzz",
    r"B:\youtubeProjects\Originals\series\ep1\buzz\rigs\Guddu.buzz",
    r"B:\youtubeProjects\Originals\series\ep1\buzz\rigs\Woman1.buzz",
    PUPPET,
];

/// The rig argument that means "the director's procedural puppet" rather
/// than a file.
const PUPPET: &str = "puppet";

/// The cells: a label, the gesture (`None` for the rest pose) and the frame
/// of the 36 to render.
const CELLS: [(&str, Option<Action>, u32); 16] = [
    ("REST", None, 0),
    ("WAVE A", Some(Action::Wave), 15),
    ("WAVE B", Some(Action::Wave), 21),
    ("NOD DOWN", Some(Action::Nod), 9),
    ("NOD UP", Some(Action::Nod), 18),
    ("SHAKE A", Some(Action::HeadShake), 6),
    ("SHAKE B", Some(Action::HeadShake), 18),
    ("SHRUG", Some(Action::Shrug), 18),
    ("SHRUG IN", Some(Action::Shrug), 5),
    ("NAMASTE IN", Some(Action::Namaste), 5),
    ("NAMASTE", Some(Action::Namaste), 18),
    ("POINT", Some(Action::Point), 18),
    ("REACH", Some(Action::Reach), 18),
    ("REACT", Some(Action::React), 6),
    ("TALK", Some(Action::Talk), 4),
    ("TALK B", Some(Action::Talk), 18),
];

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

/// The first visible armature on frame 0, as the episode builder finds a
/// Cast rig's body.
fn body_of(scene: &Scene) -> Option<ObjectId> {
    scene.layers().iter().find_map(|layer| {
        layer
            .objects_at(0)
            .iter()
            .find(|o| o.visible && matches!(o.kind, ObjectKind::Armature(_)))
            .map(|o| o.id)
    })
}

fn dump_bones(scene: &Scene, body: ObjectId) {
    let Some((_, object)) = scene.find_object(body) else {
        return;
    };
    let ObjectKind::Armature(rig) = &object.kind else {
        return;
    };
    let a = &rig.armature;
    for (i, bone) in a.bones.iter().enumerate() {
        let head = a.head(i);
        let tip = a.tip(i);
        println!(
            "  {i:2} {:<14} parent {:<5} len {:6.1} rest {:7.1} angle {:7.1} world {:7.1} head ({:6.1},{:6.1}) tip ({:6.1},{:6.1}) limits {}",
            bone.name,
            format!("{:?}", bone.parent),
            bone.length,
            bone.rest_angle.to_degrees(),
            bone.angle.to_degrees(),
            a.world_angle(i).to_degrees(),
            head.x,
            head.y,
            tip.x,
            tip.y,
            bone.limits.map_or("none".to_string(), |l| format!(
                "{:.0}..{:.0}",
                l.min.to_degrees(),
                l.max.to_degrees()
            )),
        );
    }
    println!(
        "  object transform {:?}",
        object.transform.as_coeffs().map(|v| (v * 1000.0).round() / 1000.0)
    );
}

/// The procedural figure as the director stages it — `puppet::build`, facing
/// right, standing on the ground of a default stage — with the live breath
/// and blink off so the sheet shows the gesture and nothing else.
fn puppet_scene() -> (Scene, ObjectId) {
    let mut scene = Scene::default();
    let puppet = buzz_act::puppet::build(
        &mut scene,
        &buzz_act::puppet::PuppetSpec {
            name: "Puppet".into(),
            at: buzz_geom::Point::new(640.0, 600.0),
            blink_rate: 0.0,
            breathe_rate: 0.0,
            frames: FRAMES,
            ..Default::default()
        },
    );
    (scene, puppet.body)
}

/// `--layers`: the cells of the bone-against-layer sheet. Walk is written
/// on the spot (no travel) so it stays in the cell.
const PAIR_CELLS: [(&str, Option<Action>, u32); 6] = [
    ("REST", None, 0),
    ("WAVE", Some(Action::Wave), 15),
    ("SHRUG", Some(Action::Shrug), 18),
    ("POINT", Some(Action::Point), 18),
    ("TALK", Some(Action::Talk), 4),
    ("WALK", Some(Action::Walk), 9),
];

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

/// **LR1: the rig's bone version above its layer version** (Bones → Layers on
/// a copy in memory; the file is not touched), the same performance written
/// on each with `perform::apply`, one sheet per rig,
/// `<out-dir>/layers-<rig>.png`.
fn layer_sheets(out: &std::path::Path, rigs: &[String]) -> anyhow::Result<()> {
    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    for path in rigs.iter().filter(|p| !p.eq_ignore_ascii_case(PUPPET)) {
        let (scenes, active) =
            buzz_doc::format::load_scenes(path).map_err(|e| anyhow::anyhow!("{path}: {e:?}"))?;
        let Some((_, mut base)) = scenes.into_iter().nth(active) else {
            anyhow::bail!("{path}: no active scene");
        };
        let Some(body) = body_of(&base) else {
            anyhow::bail!("{path}: no armature on frame 0");
        };
        let name = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "rig".to_string());
        base.stage_mut().background =
            peniko::Color::from_rgb8(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2]);

        let mut layered = base.clone();
        let rig = buzz_app::rigging::bones_to_layers(&mut layered, 0, body)
            .ok_or_else(|| anyhow::anyhow!("{name}: the rig has skinned parts and cannot be layers"))?;
        let layer_body = layered
            .layers()
            .get(rig.root)
            .and_then(|l| l.objects_at(0).first())
            .map(|o| o.id)
            .ok_or_else(|| anyhow::anyhow!("{name}: no root part"))?;
        println!("{name}: {} part layers, root {:?}", rig.layers.len(), rig.root);

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

        let columns = PAIR_CELLS.len() as u32;
        let mut sheet = RgbaImage::from_pixel(
            columns * CELL_W,
            2 * CELL_H + 40,
            Rgba([BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]),
        );
        label(&mut sheet, &format!("{name} - bones above, layers below"), 12, 8, 4);
        for (k, (title, action, frame)) in PAIR_CELLS.iter().enumerate() {
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
                paste_cell(
                    &mut exporter,
                    &mut sheet,
                    &scene,
                    *frame,
                    region,
                    at,
                    &format!("{kind} {title} F{frame}"),
                )?;
            }
        }
        let file = out.join(format!("layers-{name}.png"));
        sheet.save(&file)?;
        println!("wrote {}", file.display());
    }
    Ok(())
}

/// `--walk`: one cycle of Walk and of Run, eight evenly spaced frames each,
/// the bone rig above its layer version (`walk-<rig>.png`). Written on the
/// spot (no travel) so every frame stays in its cell.
///
/// The performance is four cycles long and the frames are taken from the
/// third, where the travelling beat's ease (`perform::eased_progress`, whose
/// trapezoid this repeats) has reached its steady rate, so the phases are
/// evenly spaced. Each cell is labelled with its frame and its phase in
/// hundredths of a cycle.
fn walk_sheets(out: &std::path::Path, rigs: &[String]) -> anyhow::Result<()> {
    const CYCLES: f64 = 4.0;
    const SHOTS: u32 = 8;
    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    for path in rigs.iter().filter(|p| !p.eq_ignore_ascii_case(PUPPET)) {
        let (scenes, active) =
            buzz_doc::format::load_scenes(path).map_err(|e| anyhow::anyhow!("{path}: {e:?}"))?;
        let Some((_, mut base)) = scenes.into_iter().nth(active) else {
            anyhow::bail!("{path}: no active scene");
        };
        let Some(body) = body_of(&base) else {
            anyhow::bail!("{path}: no armature on frame 0");
        };
        let name = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "rig".to_string());
        base.stage_mut().background =
            peniko::Color::from_rgb8(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2]);
        let view = match base.find_object(body).map(|(_, o)| &o.kind) {
            Some(ObjectKind::Armature(rig)) => perform::rig_view(&rig.armature.at_rest()),
            _ => perform::RigView::Side,
        };
        println!("{name}: view {view:?}");

        let mut layered = base.clone();
        let rig = buzz_app::rigging::bones_to_layers(&mut layered, 0, body)
            .ok_or_else(|| anyhow::anyhow!("{name}: the rig has skinned parts and cannot be layers"))?;
        let layer_body = layered
            .layers()
            .get(rig.root)
            .and_then(|l| l.objects_at(0).first())
            .map(|o| o.id)
            .ok_or_else(|| anyhow::anyhow!("{name}: no root part"))?;

        let (bounds, cx) = {
            let (_, object) = base.find_object(body).expect("the body");
            let cx = match &object.kind {
                ObjectKind::Armature(rig) => (object.transform * rig.armature.head(0)).x,
                _ => base.resolved_bounds(object).center().x,
            };
            (base.resolved_bounds(object), cx)
        };
        let aspect = CELL_W as f64 / (CELL_H - LABEL_H) as f64;
        let mut height = bounds.height() * 1.25;
        let mut width = height * aspect;
        if width < bounds.width() * 1.5 {
            width = bounds.width() * 1.5;
            height = width / aspect;
        }
        let top = bounds.y1 + bounds.height() * 0.06 - height;
        let region = kurbo::Rect::new(cx - width / 2.0, top, cx + width / 2.0, top + height);

        let fps = base.stage().frame_rate.max(1.0);
        let mut sheet = RgbaImage::from_pixel(
            SHOTS * CELL_W,
            4 * CELL_H + 40,
            Rgba([BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]),
        );
        label(&mut sheet, &format!("{name} - {view:?} view - walk and run"), 12, 8, 4);
        for (pair, action) in [Action::Walk, Action::Run].into_iter().enumerate() {
            let frames = (CYCLES * fps * action.cycle_seconds()).round() as u32;
            let ramp = (0.5 / CYCLES).min(0.25);
            for (row, (source, who, kind)) in
                [(&base, body, "BONES"), (&layered, layer_body, "LAYERS")].into_iter().enumerate()
            {
                let mut scene = source.clone();
                let mut performance = Performance::new(action, 0..frames);
                performance.distance = 0.0;
                let report = perform::apply(&mut scene, who, &performance)
                    .map_err(|e| anyhow::anyhow!("{name} {kind}: {e}"))?;
                assert_eq!(report.cycles as f64, CYCLES, "{name}: the cycles did not come out at four");
                for k in 0..SHOTS {
                    let want = 2.0 + k as f64 / SHOTS as f64;
                    let u = want / CYCLES * (1.0 - ramp) + ramp / 2.0;
                    let frame = (u * frames as f64).round() as u32;
                    let got = ((frame as f64 / frames as f64 - ramp / 2.0) / (1.0 - ramp) * CYCLES)
                        .rem_euclid(1.0);
                    let at = (k * CELL_W, 40 + (2 * pair + row) as u32 * CELL_H);
                    paste_cell(
                        &mut exporter,
                        &mut sheet,
                        &scene,
                        frame,
                        region,
                        at,
                        &format!(
                            "{} {kind} F{frame} P{:02}",
                            action.label(),
                            (got * 100.0).round() as u32 % 100
                        ),
                    )?;
                }
            }
        }
        let file = out.join(format!("walk-{name}.png"));
        sheet.save(&file)?;
        println!("wrote {}", file.display());
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".to_string()));
    std::fs::create_dir_all(&out)?;
    let mut rigs: Vec<String> = args.collect();
    // `--layers` anywhere after the out-dir: the bone-against-layer sheets.
    let layers_mode = rigs.iter().any(|a| a == "--layers");
    rigs.retain(|a| a != "--layers");
    // `--walk`: one cycle of walk and run per rig (`walk_sheets`).
    let walk_mode = rigs.iter().any(|a| a == "--walk");
    rigs.retain(|a| a != "--walk");
    if rigs.is_empty() {
        rigs = DEFAULT_RIGS.iter().map(|s| s.to_string()).collect();
    }
    if walk_mode {
        return walk_sheets(&out, &rigs);
    }
    if layers_mode {
        return layer_sheets(&out, &rigs);
    }

    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    let columns = 4u32;
    let rows = (CELLS.len() as u32).div_ceil(columns);

    for path in &rigs {
        let (mut base, body, name) = if path.eq_ignore_ascii_case(PUPPET) {
            let (scene, body) = puppet_scene();
            (scene, body, "Puppet".to_string())
        } else {
            let (scenes, active) = buzz_doc::format::load_scenes(path)
                .map_err(|e| anyhow::anyhow!("{path}: {e:?}"))?;
            let Some((_, base)) = scenes.into_iter().nth(active) else {
                anyhow::bail!("{path}: no active scene");
            };
            let Some(body) = body_of(&base) else {
                anyhow::bail!("{path}: no armature on frame 0");
            };
            let name = std::path::Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "rig".to_string());
            (base, body, name)
        };
        let mirrored = match base.find_object(body).map(|(_, o)| &o.kind) {
            Some(ObjectKind::Armature(rig)) => {
                perform::arms_mirrored(&rig.armature.at_rest())
            }
            _ => false,
        };
        println!("{name}: body {body:?}, arms mirrored from the puppet's: {mirrored}");
        dump_bones(&base, body);

        base.stage_mut().background =
            peniko::Color::from_rgb8(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2]);
        // Frame the figure with room above and either side for raised arms.
        let bounds = {
            let (_, object) = base.find_object(body).expect("the body");
            base.resolved_bounds(object)
        };
        let aspect = CELL_W as f64 / (CELL_H - LABEL_H) as f64;
        let mut height = bounds.height() * 1.35;
        let mut width = height * aspect;
        if width < bounds.width() * 1.7 {
            width = bounds.width() * 1.7;
            height = width / aspect;
        }
        // Centred on the hips, not the drawing's box, which a prop or a
        // stray mark can pull to one side.
        let cx = {
            let (_, object) = base.find_object(body).expect("the body");
            match &object.kind {
                ObjectKind::Armature(rig) => (object.transform * rig.armature.head(0)).x,
                _ => bounds.center().x,
            }
        };
        let top = bounds.y1 + bounds.height() * 0.06 - height;
        let region = kurbo::Rect::new(cx - width / 2.0, top, cx + width / 2.0, top + height);

        let mut sheet = RgbaImage::from_pixel(
            columns * CELL_W,
            rows * CELL_H + 40,
            Rgba([BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]),
        );
        label(&mut sheet, &name, 12, 8, 4);

        for (k, (title, action, frame)) in CELLS.iter().enumerate() {
            let mut scene = base.clone();
            if let Some(action) = action {
                let performance = Performance::new(*action, 0..FRAMES);
                perform::apply(&mut scene, body, &performance)
                    .map_err(|e| anyhow::anyhow!("{name} {title}: {e}"))?;
            }
            let mut settings = ExportSettings::for_stage(&scene);
            settings.width = CELL_W;
            settings.height = CELL_H - LABEL_H;
            settings.region = Some(region);
            let picture = exporter.render(&scene, *frame, &settings)?;
            let cell = RgbaImage::from_raw(picture.width, picture.height, picture.pixels)
                .ok_or_else(|| anyhow::anyhow!("the rendered cell has the wrong buffer size"))?;
            let (col, row) = (k as u32 % columns, k as u32 / columns);
            let (x0, y0) = (col * CELL_W, 40 + row * CELL_H);
            for (x, y, p) in cell.enumerate_pixels() {
                // Over the background, in case anything came back translucent.
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
            label(&mut sheet, &format!("{title} F{frame}"), x0 + 8, y0 + 6, 2);
            // A thin rule round the cell.
            for x in x0..x0 + CELL_W {
                sheet.put_pixel(x, y0, Rgba([0xC8, 0xC4, 0xBC, 255]));
            }
            for y in y0..y0 + CELL_H {
                sheet.put_pixel(x0, y, Rgba([0xC8, 0xC4, 0xBC, 255]));
            }
        }
        let file = out.join(format!("gestures-{name}.png"));
        sheet.save(&file)?;
        println!("wrote {}", file.display());
    }
    Ok(())
}
