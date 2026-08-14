//! Bitmaps on the real GPU: the right pixels, in the right places.
//!
//! An image fill is placed by a brush transform Vello composes on the GPU,
//! while the path it fills is pre-transformed on the CPU — two routes to the
//! same pixel, exactly as a gradient is. So the assertions are about *where*
//! each part of the picture lands, not merely that something was drawn: an
//! image that is upside down, mirrored, or tiled a thousand times over still
//! passes "is it coloured".
//!
//! Skips cleanly when no GPU is available.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{FillSpec, ImageAsset, ImageFill, ImageId, LayerKind, Scene, ShapeData};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();
    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping bitmap test: no usable GPU ({e})");
            None
        }
    });
    match shared {
        Some(mutex) => test(&mut mutex.lock().unwrap_or_else(|e| e.into_inner())),
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// A 4x4 image with one solid colour per quadrant, so orientation is visible.
///
/// Red top-left, green top-right, blue bottom-left, yellow bottom-right. Any
/// flip or transpose swaps two of them, which no "is it coloured" check would
/// notice and every one of these will.
fn quadrants() -> Arc<ImageAsset> {
    let mut pixels = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4u32 {
        for x in 0..4u32 {
            let c: [u8; 4] = match (x < 2, y < 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 0, 255],
            };
            pixels.extend_from_slice(&c);
        }
    }
    Arc::new(ImageAsset {
        id: ImageId(1),
        name: "Quadrants".into(),
        source: Arc::new(Vec::new()),
        format: "png".into(),
        width: 4,
        height: 4,
        pixels: Arc::new(pixels),
    })
}

/// A stage with the image filling a rectangle.
fn staged(asset: Arc<ImageAsset>, area: Rect) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    let mut fill = ImageFill::new(asset, area);
    // Nearest sampling, so a four-pixel image has hard quadrant edges and the
    // assertions below are not reading a blend of two quadrants.
    fill.smooth = false;
    scene.add_shape(
        layer,
        ShapeData {
            path: area.to_path(1e-9),
            fill: Some(FillSpec::image(fill)),
            stroke: None,
            blend: buzz_scene::PaintBlend::Normal,
        },
    );
    scene
}

fn render(scene: &Scene, exporter: &mut Exporter) -> Frame {
    exporter
        .render(scene, 0, &ExportSettings::for_stage(scene))
        .expect("render")
}

fn near(pixel: [u8; 4], want: [u8; 3]) -> bool {
    let close = |a: u8, b: u8| a.abs_diff(b) <= 24;
    close(pixel[0], want[0]) && close(pixel[1], want[1]) && close(pixel[2], want[2])
}

/// **The picture is the right way up and the right way round.**
#[test]
fn a_bitmap_fills_its_shape_in_the_right_orientation() {
    with_exporter(|exporter| {
        let area = Rect::new(100.0, 50.0, 500.0, 350.0);
        let scene = staged(quadrants(), area);
        let frame = render(&scene, exporter);

        // A quarter into each quadrant of the placed rectangle.
        let (l, r) = (200, 400);
        let (t, b) = (125, 275);
        assert!(
            near(frame.pixel(l, t), [255, 0, 0]),
            "top-left should be red, got {:?}",
            frame.pixel(l, t)
        );
        assert!(
            near(frame.pixel(r, t), [0, 255, 0]),
            "top-right should be green, got {:?} — the image is mirrored",
            frame.pixel(r, t)
        );
        assert!(
            near(frame.pixel(l, b), [0, 0, 255]),
            "bottom-left should be blue, got {:?} — the image is upside down",
            frame.pixel(l, b)
        );
        assert!(
            near(frame.pixel(r, b), [255, 255, 0]),
            "bottom-right should be yellow, got {:?}",
            frame.pixel(r, b)
        );
    });
}

/// It fills **exactly** its shape: nothing outside, and no tiling inside.
///
/// Vello's default extend mode repeats an image past its edges. A shape larger
/// than the placement would then show the picture over and over, which reads
/// as a rendering fault rather than a setting.
#[test]
fn a_bitmap_does_not_tile_or_spill_outside_its_shape() {
    with_exporter(|exporter| {
        let area = Rect::new(100.0, 50.0, 300.0, 250.0);
        let scene = staged(quadrants(), area);
        let frame = render(&scene, exporter);

        // Outside the shape is the white stage.
        for (x, y) in [(50, 150), (400, 150), (200, 20), (200, 320)] {
            assert!(
                near(frame.pixel(x, y), [255, 255, 255]),
                "({x}, {y}) is outside the shape and should be background, got {:?}",
                frame.pixel(x, y)
            );
        }

        // And inside there are exactly four regions, not sixteen: the middle of
        // each quadrant is its own colour.
        assert!(near(frame.pixel(150, 100), [255, 0, 0]));
        assert!(near(frame.pixel(250, 200), [255, 255, 0]));
    });
}

/// **Cutting a bitmap keeps the picture where it was.**
///
/// This is the whole point of images being a fill: removing part of the shape
/// is a boolean on the path, and the photograph does not slide about inside
/// what is left. It is what makes "magic-wand the sky and delete it" produce a
/// usable asset rather than a rearranged one.
#[test]
fn cutting_the_shape_leaves_the_picture_where_it_was() {
    with_exporter(|exporter| {
        let area = Rect::new(100.0, 50.0, 500.0, 350.0);
        let mut scene = staged(quadrants(), area);

        let before = render(&scene, exporter);
        let sample = (200u32, 125u32); // deep inside the red quadrant
        assert!(near(before.pixel(sample.0, sample.1), [255, 0, 0]));

        // Cut the right-hand half away, as a lasso or a wand would.
        let id = scene
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0).iter().map(|o| o.id))
            .next()
            .expect("the image shape");
        scene.update_object(id, |o| {
            if let buzz_scene::ObjectKind::Shape(shape) = &mut o.kind {
                shape.path = buzz_geom::boolean(
                    &shape.path,
                    &Rect::new(300.0, 0.0, 600.0, 400.0).to_path(1e-9),
                    buzz_geom::BoolOp::Difference,
                    buzz_geom::BooleanOptions::for_shape_size(400.0),
                );
            }
        });

        let after = render(&scene, exporter);

        // The kept half still shows the same part of the picture.
        assert!(
            near(after.pixel(sample.0, sample.1), [255, 0, 0]),
            "the picture moved when the shape was cut: {:?}",
            after.pixel(sample.0, sample.1)
        );
        // And the cut half is gone.
        assert!(
            near(after.pixel(400, 125), [255, 255, 255]),
            "the cut-away half is still drawn: {:?}",
            after.pixel(400, 125)
        );
    });
}

/// A transparent image composites rather than painting its transparency black.
#[test]
fn a_transparent_bitmap_shows_what_is_behind_it() {
    with_exporter(|exporter| {
        // Half opaque red, half fully transparent.
        let mut pixels = Vec::new();
        for y in 0..2u32 {
            for _ in 0..2u32 {
                if y == 0 {
                    pixels.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    pixels.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        let asset = Arc::new(ImageAsset {
            id: ImageId(2),
            name: "Half".into(),
            source: Arc::new(Vec::new()),
            format: "png".into(),
            width: 2,
            height: 2,
            pixels: Arc::new(pixels),
        });

        let area = Rect::new(100.0, 50.0, 400.0, 350.0);
        let scene = staged(asset, area);
        let frame = render(&scene, exporter);

        assert!(
            near(frame.pixel(250, 120), [255, 0, 0]),
            "the opaque half should be red"
        );
        assert!(
            near(frame.pixel(250, 280), [255, 255, 255]),
            "the transparent half should show the white stage, got {:?}",
            frame.pixel(250, 280)
        );
    });
}
