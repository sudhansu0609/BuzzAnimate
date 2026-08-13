//! Two shapes that share an edge must meet with nothing between them.
//!
//! # The defect
//!
//! Every path is antialiased on its own, so along a shared boundary each shape
//! covers about half of every pixel — and one composited over the other gives
//! `0.5 + 0.5 × (1 − 0.5) = 0.75`, not `1.0`. The remaining quarter is the
//! stage showing through, and it draws a thin pale line along every join in the
//! drawing. Imported artwork shows it everywhere, because Flash stored a shape
//! as a soup of edges scan-converted together, so every region in a `.fla`
//! shares its boundary exactly with its neighbour.
//!
//! This is measured rather than argued about: the test renders two touching
//! rectangles on a **white** stage in two dark colours and looks for white
//! along the join. Nothing but pixels can settle it.
//!
//! Skips cleanly when no GPU is available.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{FillSpec, LayerKind, Scene, ShapeData};
use peniko::Color;

const RED: Color = Color::from_rgb8(0xC0, 0x20, 0x20);
const BLUE: Color = Color::from_rgb8(0x20, 0x20, 0xC0);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping seam test: no usable GPU ({e})");
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

/// Two rectangles meeting **exactly** on x = 200, on a white stage.
///
/// The join is deliberately at a fractional position in one variant and a whole
/// pixel in another: a seam on a whole-pixel boundary can hide, because the
/// rasteriser happens to give one shape the whole pixel. The awkward case is
/// the one that has to pass.
fn touching(split: f64) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = buzz_geom::Size::new(400.0, 200.0);

    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData {
            path: Rect::new(50.0, 40.0, split, 160.0).to_path(1e-9),
            fill: Some(FillSpec::solid(RED)),
            stroke: None,
            blend: buzz_scene::PaintBlend::Normal,
        },
    );
    scene.add_shape(
        layer,
        ShapeData {
            path: Rect::new(split, 40.0, 350.0, 160.0).to_path(1e-9),
            fill: Some(FillSpec::solid(BLUE)),
            stroke: None,
            blend: buzz_scene::PaintBlend::Normal,
        },
    );
    scene
}

/// How close a pixel is to the white stage, `0.0` (not at all) to `1.0`.
///
/// The seam is the *stage* showing through, so that is what is measured —
/// rather than "is it red or blue", which cannot tell a clean blend of the two
/// from a gap between them.
fn whiteness(pixel: [u8; 4]) -> f64 {
    let m = pixel[0].min(pixel[1]).min(pixel[2]);
    f64::from(m) / 255.0
}

fn render(scene: &Scene, exporter: &mut Exporter) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("render")
}

/// **The defect, measured.** Nowhere along the join is the stage visible.
#[test]
fn two_shapes_sharing_an_edge_leave_no_pale_line() {
    with_exporter(|exporter| {
        for split in [200.0, 200.5, 187.3] {
            let scene = touching(split);
            let frame = render(&scene, exporter);

            // Sweep the columns either side of the join, down the whole height
            // of the rectangles. The worst pixel is the one that matters: a
            // seam one column wide is exactly the complaint.
            let mut worst = 0.0f64;
            let mut worst_at = (0, 0);
            for x in (split as u32).saturating_sub(3)..=(split as u32 + 3) {
                for y in 50..150 {
                    let w = whiteness(frame.pixel(x, y));
                    if w > worst {
                        worst = w;
                        worst_at = (x, y);
                    }
                }
            }

            assert!(
                worst < 0.25,
                "a pale seam runs along the join at split {split}: \
                 the stage is {:.0}% visible at {worst_at:?}",
                worst * 100.0
            );
        }
    });
}

/// The artwork either side of the join is still its own colour — the seal must
/// close the gap without smearing one shape's colour across the other.
#[test]
fn sealing_does_not_bleed_one_colour_into_the_other() {
    with_exporter(|exporter| {
        let scene = touching(200.0);
        let frame = render(&scene, exporter);

        let left = frame.pixel(120, 100);
        let right = frame.pixel(280, 100);

        assert!(
            left[0] > 150 && left[2] < 80,
            "the left shape should still be red, got {left:?}"
        );
        assert!(
            right[2] > 150 && right[0] < 80,
            "the right shape should still be blue, got {right:?}"
        );
    });
}

/// The silhouette must not run away. Sealing grows each shape by half a pixel;
/// a bug that grew it by a document *unit* would be invisible on the seam test
/// and obvious here.
#[test]
fn the_outer_edge_moves_by_less_than_a_pixel() {
    with_exporter(|exporter| {
        let scene = touching(200.0);
        let frame = render(&scene, exporter);

        // The rectangles run from x = 50 to x = 350. Two pixels outside is
        // background whatever the seal does; two pixels inside is artwork.
        let y = 100;
        assert!(
            whiteness(frame.pixel(47, y)) > 0.9,
            "the shape has grown well past its edge on the left"
        );
        assert!(
            whiteness(frame.pixel(353, y)) > 0.9,
            "the shape has grown well past its edge on the right"
        );
        assert!(
            whiteness(frame.pixel(53, y)) < 0.25,
            "the left edge should be solid artwork"
        );
    });
}

/// **Translucent artwork is left alone**, and this is the test that pins it.
///
/// Stroking a translucent fill with its own colour composites twice around the
/// rim and draws a visible darker outline — turning a faint seam into an
/// obvious border. So translucent shapes keep the seam, and what must not
/// happen is a rim.
#[test]
fn a_translucent_shape_is_not_given_a_dark_rim() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;
        scene.stage_mut().size = buzz_geom::Size::new(400.0, 200.0);
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.add_shape(
            layer,
            ShapeData {
                path: Rect::new(100.0, 50.0, 300.0, 150.0).to_path(1e-9),
                fill: Some(FillSpec::solid(Color::from_rgba8(0, 0, 0, 128))),
                stroke: None,
                blend: buzz_scene::PaintBlend::Normal,
            },
        );
        let frame = render(&scene, exporter);

        // Half-alpha black on white is mid grey. The rim, two pixels inside the
        // edge, must be the same mid grey as the middle — not darker.
        let middle = f64::from(frame.pixel(200, 100)[0]);
        let rim = f64::from(frame.pixel(102, 100)[0]);
        assert!(
            (rim - middle).abs() < 12.0,
            "the edge is a different tone from the middle ({rim} against {middle}) \
             — a translucent fill has been sealed and drawn twice"
        );
    });
}
