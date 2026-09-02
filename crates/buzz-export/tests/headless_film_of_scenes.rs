//! **A film is its scenes, end to end** — in the file on disk, not only in the
//! panel.
//!
//! A document holds several named scenes: the shots of the film. Export used to
//! render whichever one happened to be open, so a three-shot conversation came
//! out as one shot and the only way to get the whole thing was to build it as a
//! single enormous timeline, pasting each shot onto the end of the last. That
//! is a claim about files, so it is settled the way the looping claim is:
//! render a sequence of a multi-scene film and read the colours back out of the
//! PNGs in order.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use buzz_export::{ExportSettings, Exporter, Reel};
use buzz_render::GpuPreference;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{LayerKind, LoopRegion, Scene, ShapeData};
use peniko::Color;

/// One clearly distinguishable colour per scene, so a frame can be traced back
/// to the shot it came from by its pixels alone.
const COLORS: [Color; 3] = [
    Color::from_rgb8(0xFF, 0x00, 0x00),
    Color::from_rgb8(0x00, 0xFF, 0x00),
    Color::from_rgb8(0x00, 0x00, 0xFF),
];

fn gpu_ready() -> bool {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("skipping film-of-scenes test: no usable GPU ({e})");
            false
        }
    }
}

/// One shot: `frames` long, filled edge to edge in one colour.
fn shot(color: Color, frames: u32) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), color),
    );
    if frames > 1 {
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(frames - 1);
        });
    }
    scene
}

/// Which shot this image came from, by its middle pixel.
fn shot_of(frame: &buzz_export::Frame) -> usize {
    let x = frame.width / 2;
    let y = frame.height / 2;
    let i = ((y * frame.width + x) * 4) as usize;
    let pixel = &frame.pixels[i..i + 4];

    COLORS
        .iter()
        .position(|c| {
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            let near = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 8;
            near(pixel[0], r) && near(pixel[1], g) && near(pixel[2], b)
        })
        .unwrap_or_else(|| panic!("unrecognised colour {pixel:?}"))
}

fn read_png(path: &std::path::Path) -> buzz_export::Frame {
    let file = std::fs::File::open(path)
        .unwrap_or_else(|e| panic!("{} was not written: {e}", path.display()));
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("png header");
    let mut pixels = vec![0u8; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut pixels).expect("png data");
    assert_eq!(info.color_type, png::ColorType::Rgba, "expected RGBA output");
    pixels.truncate(info.buffer_size());
    buzz_export::Frame {
        width: info.width,
        height: info.height,
        pixels,
    }
}

/// Render the whole film to a folder and report which shot each frame shows.
fn film_order(scenes: &[Scene], name: &str) -> Vec<usize> {
    let reel = Reel::of(scenes.iter());
    let dir = tempfile::tempdir().expect("temp dir");
    let settings = ExportSettings::scaled(reel.lead().expect("a lead scene"), 0.25);

    let report = buzz_export::export_sequence(
        &reel,
        0..reel.frames(),
        dir.path(),
        name,
        &settings,
        &GpuPreference::Automatic,
        |_, _| true,
    )
    .expect("the film exports");

    assert_eq!(
        report.frames,
        reel.frames(),
        "the film should be as long as all its shots together"
    );

    (0..reel.frames())
        .map(|index| shot_of(&read_png(&dir.path().join(format!("{name}{index:04}.png")))))
        .collect()
}

/// **The claim.** Three scenes of two, three and one frames export as one
/// six-frame film, in order — not as whichever scene was open.
#[test]
fn a_film_exports_every_scene_in_order() {
    if !gpu_ready() {
        return;
    }
    let scenes = [shot(COLORS[0], 2), shot(COLORS[1], 3), shot(COLORS[2], 1)];
    assert_eq!(
        film_order(&scenes, "film"),
        vec![0, 0, 1, 1, 1, 2],
        "the shots should run one after another"
    );
}

/// **A one-scene document exports exactly what it always did.** Every feature
/// here has to leave documents that do not use it alone, and this is that
/// invariant for scenes.
#[test]
fn a_single_scene_film_is_unchanged() {
    if !gpu_ready() {
        return;
    }
    let scenes = [shot(COLORS[1], 4)];
    assert_eq!(film_order(&scenes, "one"), vec![1, 1, 1, 1]);
}

/// Looping still belongs to the scene it is set on: a repeating section inside
/// the first shot repeats inside that shot, and the shot after it follows the
/// repeats rather than being pushed off the end.
#[test]
fn a_looping_scene_repeats_inside_its_own_shot() {
    if !gpu_ready() {
        return;
    }
    let mut first = shot(COLORS[0], 3);
    *first.looping_mut() = LoopRegion {
        enabled: true,
        start: 0,
        end: 2,
        repeats: 2,
    };
    let scenes = [first, shot(COLORS[2], 2)];

    // Three frames played twice, then the second shot's two.
    assert_eq!(
        film_order(&scenes, "loop"),
        vec![0, 0, 0, 0, 0, 0, 2, 2],
        "the repeats belong to the first shot and the second still follows"
    );
}
