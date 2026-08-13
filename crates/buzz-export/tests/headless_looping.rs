//! Prove the looping section is in the **finished film**, not only in the
//! preview.
//!
//! Animate's loop is a transport setting: the playhead cycles while you work,
//! and the published file knows nothing about it. Ours is part of the document
//! and the exporter repeats it — which is a claim about files on disk, so it
//! is settled by rendering a sequence and reading the colours back out of the
//! PNGs in order.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, LoopRegion, Scene, ShapeData};
use peniko::Color;

/// One clearly distinguishable colour per frame, so a frame can be identified
/// from its pixels alone.
const COLORS: [Color; 4] = [
    Color::from_rgb8(0xFF, 0x00, 0x00),
    Color::from_rgb8(0x00, 0xFF, 0x00),
    Color::from_rgb8(0x00, 0x00, 0xFF),
    Color::from_rgb8(0xFF, 0xFF, 0x00),
];

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping looping test: no usable GPU ({e})");
            None
        }
    });
    match shared {
        Some(mutex) => test(&mut mutex.lock().unwrap_or_else(|e| e.into_inner())),
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// Four frames, each a full-stage rectangle in its own colour.
fn flashcards() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);

    for (frame, color) in COLORS.iter().enumerate() {
        let frame = frame as u32;
        if frame > 0 {
            // Extend the span before keying it, as F5 then F7 does — a
            // keyframe made past the end of a span comes up blank.
            scene.update_layer(layer, |l| {
                l.frames.insert_frame(frame);
                l.frames.insert_blank_keyframe(frame);
            });
        }
        scene.add_shape_at(
            layer,
            frame,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), *color),
        );
    }
    scene
}

/// Which of the four colours this image is, by its middle pixel.
fn colour_of(frame: &Frame) -> usize {
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

/// A written frame, read back off the disk.
fn read_png(path: &std::path::Path) -> Frame {
    let file = std::fs::File::open(path)
        .unwrap_or_else(|e| panic!("{} was not written: {e}", path.display()));
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("png header");
    let mut pixels = vec![0u8; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut pixels).expect("png data");
    assert_eq!(info.color_type, png::ColorType::Rgba, "expected RGBA output");
    pixels.truncate(info.buffer_size());
    Frame {
        width: info.width,
        height: info.height,
        pixels,
    }
}

/// The document renders its own frames in order when nothing loops. Without
/// this the test below could pass on a document that renders nonsense.
#[test]
fn the_flashcards_render_in_order() {
    with_exporter(|exporter| {
        let scene = flashcards();
        let settings = ExportSettings::scaled(&scene, 0.25);
        for frame in 0..4u32 {
            let image = exporter.render(&scene, frame, &settings).expect("render");
            assert_eq!(
                colour_of(&image),
                frame as usize,
                "frame {frame} drew the wrong card"
            );
        }
    });
}

/// **The claim.** With frames 2 and 3 (one-based) set to repeat three times,
/// the exported sequence really contains them three times, in order, and the
/// frames outside the section appear exactly once.
#[test]
fn an_exported_sequence_repeats_the_looping_section() {
    if Exporter::new(&GpuPreference::Automatic).is_err() {
        eprintln!("skipping: no usable GPU");
        return;
    }

    let mut scene = flashcards();
    *scene.looping_mut() = LoopRegion {
        enabled: true,
        start: 1,
        end: 2,
        repeats: 3,
    };

    // 4 frames, with a 2-frame section played 3 times instead of once: 8.
    assert_eq!(scene.rendered_frame_count(), 8);

    let dir = tempfile::tempdir().expect("temp dir");
    let settings = ExportSettings::scaled(&scene, 0.25);
    let report = buzz_export::export_sequence(
        &scene,
        0..scene.rendered_frame_count(),
        dir.path(),
        "loop",
        &settings,
        &GpuPreference::Automatic,
        |_, _| true,
    )
    .expect("sequence");

    assert_eq!(report.frames, 8, "the film is longer than the timeline");
    assert_eq!(report.files.len(), 8);

    let mut order = Vec::new();
    for index in 0..8 {
        order.push(colour_of(&read_png(
            &dir.path().join(format!("loop{index:04}.png")),
        )));
    }

    assert_eq!(
        order,
        vec![0, 1, 2, 1, 2, 1, 2, 3],
        "the section should appear three times, between the frames either side of it"
    );
}

/// A document with no looping section exports exactly what it always did.
/// Every render feature here has to leave documents that do not use it alone,
/// and this is that invariant for the exporter.
#[test]
fn a_document_without_a_loop_is_unchanged() {
    if Exporter::new(&GpuPreference::Automatic).is_err() {
        eprintln!("skipping: no usable GPU");
        return;
    }

    let scene = flashcards();
    assert_eq!(scene.rendered_frame_count(), scene.frame_count());

    let dir = tempfile::tempdir().expect("temp dir");
    let settings = ExportSettings::scaled(&scene, 0.25);
    let report = buzz_export::export_sequence(
        &scene,
        0..4,
        dir.path(),
        "plain",
        &settings,
        &GpuPreference::Automatic,
        |_, _| true,
    )
    .expect("sequence");
    assert_eq!(report.frames, 4);

    for index in 0..4u32 {
        assert_eq!(
            colour_of(&read_png(
                &dir.path().join(format!("plain{index:04}.png"))
            )),
            index as usize,
            "frame {index} of an unlooped document"
        );
    }
}

/// A range the user typed by hand is numbered in film frames too — so asking
/// for "frames 4 to 5" of a looping document gets the fourth and fifth frames
/// of the film, which are inside a repeat.
#[test]
fn a_partial_range_is_numbered_in_film_frames() {
    if Exporter::new(&GpuPreference::Automatic).is_err() {
        eprintln!("skipping: no usable GPU");
        return;
    }

    let mut scene = flashcards();
    *scene.looping_mut() = LoopRegion {
        enabled: true,
        start: 1,
        end: 2,
        repeats: 3,
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let settings = ExportSettings::scaled(&scene, 0.25);
    // Film frames 4..6 are the second pass over the section: green, blue.
    buzz_export::export_sequence(
        &scene,
        3..5,
        dir.path(),
        "part",
        &settings,
        &GpuPreference::Automatic,
        |_, _| true,
    )
    .expect("sequence");

    let read =
        |index: u32| colour_of(&read_png(&dir.path().join(format!("part{index:04}.png"))));
    assert_eq!(read(3), 1, "film frame 4 is the second pass, first frame");
    assert_eq!(read(4), 2);
}
