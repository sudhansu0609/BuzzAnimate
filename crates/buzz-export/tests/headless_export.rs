//! Prove an export on the real GPU: the right size, the right pixels, in the
//! right places.
//!
//! The claims worth testing are the ones arithmetic cannot settle — that the
//! camera frames the stage exactly, that a width needing row padding comes
//! back un-sheared, and that a transparent export really is transparent where
//! there is no artwork. A sheared image from mishandled row alignment looks
//! like a rendering bug and is not one, so it is asserted directly.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

const RED: Color = Color::from_rgb8(0xFF, 0x00, 0x00);
const BLUE: Color = Color::from_rgb8(0x00, 0x00, 0xFF);

/// One GPU for the whole file: six devices in parallel is wasteful, and a
/// context that fails to acquire makes its test *skip*, which is worse than
/// failing.
fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping export test: no usable GPU ({e})");
            None
        }
    });

    match shared {
        Some(mutex) => {
            let mut exporter = mutex.lock().unwrap_or_else(|e| e.into_inner());
            test(&mut exporter);
        }
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// A 550 x 400 stage, white, with a blue square covering the left half of the
/// top half. That places a known colour at a known place and leaves the rest
/// background, which is what makes the assertions readable.
fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(0.0, 0.0, 275.0, 200.0).to_path(1e-9), BLUE),
    );
    scene
}

fn is(pixel: [u8; 4], colour: Color) -> bool {
    let [r, g, b, _] = colour.to_rgba8().to_u8_array();
    let close = |a: u8, b: u8| a.abs_diff(b) <= 2;
    close(pixel[0], r) && close(pixel[1], g) && close(pixel[2], b)
}

#[test]
fn an_export_is_the_size_asked_for_and_holds_the_artwork_where_it_belongs() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("render");

        assert_eq!((frame.width, frame.height), (550, 400));
        assert_eq!(frame.pixels.len(), 550 * 400 * 4);

        // The blue square covers the top-left quarter; the rest is white.
        assert!(
            is(frame.pixel(50, 50), BLUE),
            "top-left should be the square"
        );
        assert!(
            is(frame.pixel(500, 350), Color::WHITE),
            "bottom-right should be background"
        );
        assert!(
            is(frame.pixel(500, 50), Color::WHITE),
            "top-right is outside the square"
        );
        assert!(
            is(frame.pixel(50, 350), Color::WHITE),
            "bottom-left is outside the square"
        );

        // Every pixel is opaque: an ordinary export has no holes in it.
        assert!(
            frame.pixels.chunks_exact(4).all(|p| p[3] == 255),
            "an opaque export should have no transparency"
        );
    });
}

/// 550 x 4 = 2200 bytes per row, which is not a multiple of the GPU's 256-byte
/// copy alignment. Getting the padding wrong shears the image progressively,
/// so the *last* row is where it shows — and the first row would still look
/// perfect.
#[test]
fn a_width_needing_row_padding_comes_back_unsheared() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("render");

        // Bottom row, both ends: all background, and still in the right place.
        let bottom = frame.height - 1;
        for x in [0, 137, 274, 400, frame.width - 1] {
            assert!(
                is(frame.pixel(x, bottom), Color::WHITE),
                "bottom row at x={x} is not background — the rows have shifted"
            );
        }

        // The square's right edge sits at x = 275 on every row it covers. A
        // shear would move it row by row.
        for y in [0, 60, 120, 199] {
            assert!(is(frame.pixel(270, y), BLUE), "inside the square at y={y}");
            assert!(
                is(frame.pixel(280, y), Color::WHITE),
                "just past the square at y={y}"
            );
        }
    });
}

#[test]
fn scaling_up_keeps_the_composition() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings::scaled(&scene, 2.0);
        let frame = exporter.render(&scene, 0, &settings).expect("render");

        assert_eq!((frame.width, frame.height), (1100, 800));
        // The square still covers the top-left quarter, at twice the size.
        assert!(is(frame.pixel(100, 100), BLUE));
        assert!(is(frame.pixel(540, 380), BLUE), "just inside at 2x");
        assert!(
            is(frame.pixel(560, 420), Color::WHITE),
            "just outside at 2x"
        );
    });
}

/// A transparent export drops the background but keeps the artwork opaque.
#[test]
fn a_transparent_export_has_a_clear_background_and_solid_artwork() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings {
            transparent: true,
            ..ExportSettings::for_stage(&scene)
        };
        let frame = exporter.render(&scene, 0, &settings).expect("render");

        let inside = frame.pixel(50, 50);
        assert_eq!(inside[3], 255, "the artwork stays opaque");
        assert!(
            is(inside, BLUE),
            "and keeps its colour after unpremultiplying"
        );

        let outside = frame.pixel(500, 350);
        assert_eq!(outside[3], 0, "the background is gone");
    });
}

/// The frame number is honoured — an export of frame 5 must not quietly render
/// frame 0, which is the sort of thing that only shows up in the output file.
#[test]
fn a_later_frame_renders_that_frames_artwork() {
    with_exporter(|exporter| {
        let mut scene = document();
        let layer = scene.layers().iter().next().expect("a layer").id;
        scene.update_layer(layer, |l| {
            l.frames.insert_blank_keyframe(5);
        });
        scene.add_shape_at(
            layer,
            5,
            ShapeData::filled(Rect::new(300.0, 250.0, 500.0, 380.0).to_path(1e-9), RED),
        );

        let settings = ExportSettings::for_stage(&scene);

        let first = exporter.render(&scene, 0, &settings).expect("render 0");
        assert!(is(first.pixel(50, 50), BLUE), "frame 0 has the blue square");
        assert!(
            is(first.pixel(400, 300), Color::WHITE),
            "and not the red one"
        );

        let later = exporter.render(&scene, 5, &settings).expect("render 5");
        assert!(is(later.pixel(400, 300), RED), "frame 5 has the red square");
        assert!(
            is(later.pixel(50, 50), Color::WHITE),
            "and the blank keyframe cleared the blue one"
        );
    });
}

/// A round trip through the file: what is written is what was rendered.
#[test]
fn an_exported_png_decodes_back_to_the_same_pixels() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);
        let rendered = exporter.render(&scene, 0, &settings).expect("render");

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("still.png");
        rendered.write_png(&path).expect("write");

        let file = std::io::BufReader::new(std::fs::File::open(&path).expect("open"));
        let mut reader = png::Decoder::new(file).read_info().expect("read info");
        let mut buffer = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buffer).expect("decode");

        let decoded = Frame {
            width: info.width,
            height: info.height,
            pixels: buffer[..info.buffer_size()].to_vec(),
        };
        assert_eq!(decoded, rendered, "the file is not what was rendered");
    });
}

/// Refused with a message naming the limit, rather than truncated or hung.
#[test]
fn an_image_beyond_the_gpus_limit_is_refused_with_the_reason() {
    with_exporter(|exporter| {
        let scene = document();
        let limit = exporter.max_dimension();
        let settings = ExportSettings {
            width: limit + 1,
            height: 100,
            transparent: false,
        };

        let error = exporter
            .render(&scene, 0, &settings)
            .expect_err("this should be refused");
        let message = error.to_string();
        assert!(
            message.contains(&limit.to_string()),
            "the message should say what the machine can do: {message}"
        );
    });
}

/// A sequence writes one numbered file per frame, in order, and reports them.
#[test]
fn a_sequence_writes_a_numbered_file_for_every_frame() {
    // Renders through its own exporter, so this also covers the public entry
    // point the menu calls rather than only the struct.
    if Exporter::new(&GpuPreference::Automatic).is_err() {
        eprintln!("skipping: no usable GPU");
        return;
    }

    let scene = document();
    let dir = tempfile::tempdir().expect("temp dir");
    let settings = ExportSettings::scaled(&scene, 0.25);

    let mut seen = Vec::new();
    let report = buzz_export::export_sequence(
        &scene,
        0..5,
        dir.path(),
        "frame",
        &settings,
        &GpuPreference::Automatic,
        |done, total| {
            seen.push((done, total));
            true
        },
    )
    .expect("sequence");

    assert_eq!(report.frames, 5);
    assert_eq!(report.files.len(), 5);
    for frame in 0..5 {
        let path = dir.path().join(format!("frame{frame:04}.png"));
        assert!(path.exists(), "{} was not written", path.display());
    }
    assert_eq!(seen.last().map(|(_, total)| *total), Some(5));
}

// -- masking ----------------------------------------------------------------

/// A mask layer clips the layers it claims. Nothing in the model can prove
/// this: `mask_groups` resolved the *rule* long before anything drew it, and
/// for five phases masked layers rendered unclipped while every test passed.
/// Only pixels settle it.
#[test]
fn a_mask_layer_clips_the_layer_below_it() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;

        // Bottom: a blue rectangle covering the whole stage.
        let art = scene.add_layer("Art", LayerKind::Masked);
        scene.add_shape(
            art,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), BLUE),
        );
        // Above it: a mask that shows only the left quarter.
        let mask = scene.add_layer("Mask", LayerKind::Mask);
        scene.add_shape(
            mask,
            ShapeData::filled(Rect::new(0.0, 0.0, 137.0, 400.0).to_path(1e-9), RED),
        );

        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("render");

        assert!(
            is(frame.pixel(60, 200), BLUE),
            "inside the mask the artwork should show"
        );
        assert!(
            is(frame.pixel(300, 200), Color::WHITE),
            "outside the mask it should be clipped away"
        );
        assert!(
            !is(frame.pixel(60, 200), RED),
            "the mask's own artwork must not be drawn"
        );
    });
}

/// A mask made of two separate shapes shows through both of them — the case
/// even-odd filling would get wrong by punching holes where shapes overlap.
#[test]
fn a_mask_of_several_shapes_shows_through_all_of_them() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;

        let art = scene.add_layer("Art", LayerKind::Masked);
        scene.add_shape(
            art,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), BLUE),
        );
        let mask = scene.add_layer("Mask", LayerKind::Mask);
        scene.add_shape(
            mask,
            ShapeData::filled(Rect::new(20.0, 20.0, 120.0, 380.0).to_path(1e-9), RED),
        );
        scene.add_shape(
            mask,
            ShapeData::filled(Rect::new(300.0, 20.0, 400.0, 380.0).to_path(1e-9), RED),
        );

        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("render");

        assert!(is(frame.pixel(60, 200), BLUE), "through the first shape");
        assert!(is(frame.pixel(350, 200), BLUE), "through the second");
        assert!(
            is(frame.pixel(200, 200), Color::WHITE),
            "between them there is no mask, so nothing shows"
        );
    });
}

/// An inverse mask hides what it covers and leaves the rest — the exact
/// opposite of the test above, drawn with the same two shapes.
#[test]
fn an_inverse_mask_hides_what_it_covers() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;

        let art = scene.add_layer("Art", LayerKind::Masked);
        scene.add_shape(
            art,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), BLUE),
        );
        let mask = scene.add_layer("Hole", LayerKind::InverseMask);
        scene.add_shape(
            mask,
            ShapeData::filled(Rect::new(0.0, 0.0, 137.0, 400.0).to_path(1e-9), RED),
        );

        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("render");

        assert!(
            is(frame.pixel(60, 200), Color::WHITE),
            "under the mask the artwork should be punched away"
        );
        assert!(
            is(frame.pixel(300, 200), BLUE),
            "everywhere else it should show"
        );
        assert!(
            !is(frame.pixel(60, 200), RED),
            "the mask's own artwork must not be drawn, either way round"
        );
    });
}

/// Two overlapping blobs in an inverse mask cut *one* hole.
///
/// This is the case the obvious implementation gets wrong: reversing the
/// subpaths inside a big rectangle makes the overlap wind back to filled, and
/// the middle of the hole would show the artwork again.
#[test]
fn overlapping_shapes_in_an_inverse_mask_cut_one_hole() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;

        let art = scene.add_layer("Art", LayerKind::Masked);
        scene.add_shape(
            art,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), BLUE),
        );
        let mask = scene.add_layer("Hole", LayerKind::InverseMask);
        scene.add_shape(
            mask,
            ShapeData::filled(Rect::new(40.0, 40.0, 260.0, 360.0).to_path(1e-9), RED),
        );
        scene.add_shape(
            mask,
            ShapeData::filled(Rect::new(160.0, 40.0, 380.0, 360.0).to_path(1e-9), RED),
        );

        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("render");

        assert!(
            is(frame.pixel(200, 200), Color::WHITE),
            "where the two shapes overlap the hole must still be a hole"
        );
        assert!(is(frame.pixel(80, 200), Color::WHITE), "and in the first");
        assert!(is(frame.pixel(340, 200), Color::WHITE), "and in the second");
        assert!(
            is(frame.pixel(460, 200), BLUE),
            "beyond both, the artwork is untouched"
        );
    });
}

/// A masked layer inside a symbol is clipped by that symbol's own mask,
/// wherever the instance is placed.
#[test]
fn a_mask_inside_a_symbol_clips_its_instance() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;

        let symbol = scene.add_symbol("Porthole", buzz_scene::SymbolKind::Graphic, None);
        let art = scene
            .library()
            .get(symbol)
            .expect("the symbol")
            .layers
            .iter()
            .next()
            .expect("a layer")
            .id;

        // Inside the symbol: artwork on the existing layer, a mask above it.
        scene.library_mut().update(symbol, |s| {
            s.layers.update(art, |l| {
                l.kind = LayerKind::Masked;
                l.frames.set_objects(
                    0,
                    vec![std::sync::Arc::new(buzz_scene::Object::shape(
                        buzz_scene::ObjectId(9001),
                        ShapeData::filled(Rect::new(0.0, 0.0, 200.0, 200.0).to_path(1e-9), BLUE),
                    ))],
                );
            });
            let mask_layer = buzz_scene::Layer {
                kind: LayerKind::Mask,
                ..buzz_scene::Layer::normal(buzz_scene::LayerId(9002), "Mask")
            };
            s.layers.push_front(mask_layer);
            s.layers.update(buzz_scene::LayerId(9002), |l| {
                l.frames.set_objects(
                    0,
                    vec![std::sync::Arc::new(buzz_scene::Object::shape(
                        buzz_scene::ObjectId(9003),
                        ShapeData::filled(Rect::new(0.0, 0.0, 60.0, 200.0).to_path(1e-9), RED),
                    ))],
                );
            });
        });

        let stage_layer = scene.layers().iter().next().expect("a stage layer").id;
        scene.add_instance_at(
            stage_layer,
            0,
            symbol,
            buzz_geom::Affine::translate((100.0, 100.0)),
        );

        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("render");

        // The instance sits at (100, 100); its mask shows only its left 60 units.
        assert!(is(frame.pixel(130, 200), BLUE), "inside the symbol's mask");
        assert!(
            is(frame.pixel(250, 200), Color::WHITE),
            "the rest of the symbol's artwork is masked away"
        );
    });
}

/// **A mask layer never paints itself into the finished film.**
///
/// A mask is a stencil. It used to be skipped only when it was actually
/// clipping something, which meant a mask layer added before the layer beneath
/// it had been set to Masked was drawn as ordinary artwork — opaque, full size,
/// over everything. On a vignette or a torchlight cone that is the whole frame
/// covered by a shape nobody drew to be seen, and the natural order of work
/// (draw the mask, then set what it masks) walks straight into it.
///
/// Found while building a night scene: the vignette's stencil covered the film.
#[test]
fn a_mask_that_clips_nothing_is_still_not_drawn() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;

        // Artwork underneath, on an ordinary layer.
        let art = scene.add_layer("Art", LayerKind::Normal);
        scene.add_shape(
            art,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), BLUE),
        );

        // A mask on top that claims nothing, because nothing below it has been
        // set to Masked yet.
        let mask = scene.add_layer("Stencil", LayerKind::Mask);
        scene.add_shape(
            mask,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), RED),
        );

        let frame = exporter
            .render(&scene, 0, &ExportSettings::for_stage(&scene))
            .expect("render");

        assert!(
            is(frame.pixel(275, 200), BLUE),
            "the stencil painted itself over the artwork: {:?}",
            frame.pixel(275, 200)
        );
    });
}

/// The same for an inverse mask, which is the one a vignette actually uses.
#[test]
fn an_inverse_mask_that_clips_nothing_is_not_drawn_either() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;

        let art = scene.add_layer("Art", LayerKind::Normal);
        scene.add_shape(
            art,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), BLUE),
        );
        let mask = scene.add_layer("Hole", LayerKind::InverseMask);
        scene.add_shape(
            mask,
            ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), RED),
        );

        let frame = exporter
            .render(&scene, 0, &ExportSettings::for_stage(&scene))
            .expect("render");
        assert!(
            is(frame.pixel(275, 200), BLUE),
            "the inverse stencil painted itself: {:?}",
            frame.pixel(275, 200)
        );
    });
}
