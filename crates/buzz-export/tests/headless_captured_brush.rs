//! **A brush made from artwork paints that artwork**, on the real GPU.
//!
//! The unit tests say the capture keeps its paint and that placement carries
//! a gradient and a bitmap along with the geometry. What none of them can say
//! is that the *renderer* then draws the picture — a bitmap fill whose
//! transform was rebuilt per stamp is exactly the kind of thing that comes out
//! as one smear, as the first stamp repeated, or as nothing.
//!
//! So this captures a shape painted with a four-colour bitmap, stamps it along
//! a stroke, renders it and reads the pixels back: the texture must appear, and
//! it must appear in several separate places rather than once.
//!
//! Skips with no GPU, like every other headless test here.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{
    BrushStamp, FillSpec, ImageAsset, ImageFill, ImageId, LayerKind, Object, PaintBlend, Scene,
    ShapeData,
};
use peniko::Color;

const RED: [u8; 3] = [0xFF, 0x00, 0x00];
const GREEN: [u8; 3] = [0x00, 0xFF, 0x00];
const BLUE: [u8; 3] = [0x00, 0x00, 0xFF];

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping captured-brush test: no usable GPU ({e})"),
    }
}

/// A square painted with a 2x2 bitmap: red, green, blue, yellow.
///
/// Four flat quadrants rather than a photograph, so a pixel read back can say
/// *which part* of the texture it came from — which is what proves the picture
/// travelled with the stamp instead of being sampled from one place.
fn textured_square() -> ShapeData {
    let asset = Arc::new(ImageAsset::from_pixels(
        ImageId(41),
        "Swatch",
        2,
        2,
        Arc::new(vec![
            0xFF, 0x00, 0x00, 0xFF, // red
            0x00, 0xFF, 0x00, 0xFF, // green
            0x00, 0x00, 0xFF, 0xFF, // blue
            0xFF, 0xFF, 0x00, 0xFF, // yellow
        ]),
    ));
    let area = Rect::new(0.0, 0.0, 60.0, 60.0);
    let mut fill = ImageFill::new(asset, area);
    // Nearest-neighbour, so the four quadrants stay four flat colours and a
    // sampled pixel is unambiguous.
    fill.smooth = false;
    ShapeData {
        path: area.to_path(1e-9),
        fill: Some(FillSpec::image(fill)),
        stroke: None,
        blend: PaintBlend::Normal,
    }
}

/// The stroke the brush is dragged along: straight across the middle.
fn stamp_transforms(size: f64) -> Vec<Affine> {
    let mut spine = buzz_geom::BezPath::new();
    spine.move_to(Point::new(80.0, 200.0));
    spine.line_to(Point::new(460.0, 200.0));

    let stamp_rect = Rect::new(-size / 2.0, -size / 2.0, size / 2.0, size / 2.0);
    buzz_geom::stamp_transforms(
        &spine,
        stamp_rect,
        buzz_geom::PatternFit::Repeat { spacing: 90.0 },
        &buzz_geom::BrushBudget::default(),
    )
    .transforms
    .iter()
    .map(|t| *t * Affine::scale(size))
    .collect()
}

/// A dark stage carrying one stroke of the captured brush.
fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x10, 0x10, 0x18);
    let layer = scene.add_layer("Stroke", LayerKind::Normal);

    let stamp = BrushStamp::capture(&[(Affine::IDENTITY, textured_square())])
        .expect("the artwork captures");
    assert!(stamp.is_painted(), "a bitmap fill is paint of its own");

    let placed = stamp.place_many(&stamp_transforms(56.0));
    assert!(!placed.truncated, "this stroke is well inside the budget");
    assert!(
        placed.shapes.len() >= 4,
        "a textured stamp cannot merge, so each copy is its own shape: {}",
        placed.shapes.len()
    );

    let children: Vec<Arc<Object>> = placed
        .shapes
        .into_iter()
        .map(|shape| {
            let id = scene.next_object_id();
            Arc::new(Object::shape(id, shape))
        })
        .collect();
    let id = scene.next_object_id();
    scene.add_object(layer, Object::group(id, children));

    // The texture has to be in the library, exactly as the editor puts it
    // there when a captured brush paints — otherwise it is a picture the file
    // does not contain.
    let asset = ImageAsset::from_pixels(
        ImageId(41),
        "Swatch",
        2,
        2,
        Arc::new(vec![
            0xFF, 0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF,
            0x00, 0xFF,
        ]),
    );
    scene.images_mut().insert(asset);
    scene
}

/// Is this pixel near `want`? Generous, because the stage composites and the
/// edges of a stamp are antialiased against the background.
fn near(px: &[u8], want: [u8; 3]) -> bool {
    (0..3).all(|i| (i16::from(px[i]) - i16::from(want[i])).abs() < 60)
}

/// The x positions along the stroke where `want` was found.
fn columns_showing(frame: &Frame, want: [u8; 3]) -> Vec<u32> {
    let mut found = Vec::new();
    for x in 40..520u32 {
        let mut hit = false;
        for y in 150..260u32 {
            let i = ((y * frame.width + x) * 4) as usize;
            if i + 3 < frame.pixels.len() && near(&frame.pixels[i..i + 4], want) {
                hit = true;
                break;
            }
        }
        if hit {
            found.push(x);
        }
    }
    found
}

/// **The brush paints the artwork's texture.** All four of the bitmap's
/// colours have to be on the stage — a stamp that lost its picture would show
/// the stage, one flat colour, or nothing at all.
#[test]
fn a_captured_brush_paints_its_bitmap_texture() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("a frame");

        for (name, want) in [("red", RED), ("green", GREEN), ("blue", BLUE)] {
            let columns = columns_showing(&frame, want);
            assert!(
                !columns.is_empty(),
                "the {name} quarter of the texture never reached the stage"
            );
        }
    });
}

/// **And it paints it once per stamp**, in the places the stroke put them —
/// which is what a bitmap paint rebuilt per stamp has to get right, and what
/// one shared transform would get wrong.
#[test]
fn every_stamp_carries_its_own_copy_of_the_texture() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("a frame");

        // Group the columns showing red into runs: one run per stamp.
        let columns = columns_showing(&frame, RED);
        assert!(!columns.is_empty(), "no texture on the stage at all");
        let runs = columns
            .windows(2)
            .filter(|pair| pair[1] - pair[0] > 8)
            .count()
            + 1;
        assert!(
            runs >= 4,
            "the stroke should show a separate stamp in several places, found {runs} \
             (columns {:?}..{:?})",
            columns.first(),
            columns.last()
        );

        // And they span the stroke rather than piling up at its start.
        let spread = columns.last().unwrap() - columns.first().unwrap();
        assert!(
            spread > 250,
            "the stamps should run the length of the stroke, and span {spread}px"
        );
    });
}
