//! Prove 3D rotation on the real GPU.
//!
//! The claim is that a flat drawing can be *turned* — given its own plane in
//! space — so that a camera moving past it discovers a shape instead of sliding
//! a card. These render frames and read them back to check it.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{CameraKey, LayerKind, ObjectId, Scene, ShapeData, Spatial};
use peniko::Color;

const ART: Color = Color::from_rgb8(0x20, 0x50, 0xC0);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping 3D test: no usable GPU ({e})");
            None
        }
    });
    match shared {
        Some(mutex) => test(&mut mutex.lock().unwrap_or_else(|e| e.into_inner())),
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// A white stage with one upright card in the middle.
fn document() -> (Scene, ObjectId) {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    let id = scene
        .add_shape(
            layer,
            ShapeData::filled(Rect::new(175.0, 80.0, 375.0, 320.0).to_path(1e-9), ART),
        )
        .expect("the card");
    (scene, id)
}

fn render(exporter: &mut Exporter, scene: &Scene) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("render")
}

fn is_art(pixel: [u8; 4]) -> bool {
    pixel[2] as i32 > pixel[0] as i32 + 20
}

/// How tall the artwork is on one column of pixels.
fn height_at(frame: &Frame, x: u32) -> u32 {
    (0..frame.height).filter(|y| is_art(frame.pixel(x, *y))).count() as u32
}

/// How wide it is on one row.
fn width_at(frame: &Frame, y: u32) -> u32 {
    (0..frame.width).filter(|x| is_art(frame.pixel(*x, y))).count() as u32
}

/// The promise every document depends on: a flat object is untouched.
#[test]
fn a_flat_object_renders_exactly_as_before() {
    with_exporter(|exporter| {
        let (scene, id) = document();
        let before = render(exporter, &scene);

        // Turned and flattened again leaves no trace.
        let mut touched = scene.clone();
        touched.update_object(id, |o| {
            o.spatial = Spatial {
                rotation_y: 0.4,
                ..Default::default()
            }
        });
        touched.update_object(id, |o| o.spatial = Spatial::default());

        assert_eq!(before.pixels, render(exporter, &touched).pixels);
    });
}

/// The point of it: turn a card about the vertical and one side is drawn
/// shorter than the other — it is standing at an angle, not merely scaled.
#[test]
fn turning_a_card_foreshortens_one_side() {
    with_exporter(|exporter| {
        let (mut scene, id) = document();
        scene.update_object(id, |o| {
            o.spatial = Spatial {
                rotation_y: 0.9,
                ..Default::default()
            }
        });

        let frame = render(exporter, &scene);
        // Sample near each end of what is drawn, rather than at fixed columns:
        // the card no longer fills its old footprint.
        let columns: Vec<u32> = (0..frame.width).filter(|x| height_at(&frame, *x) > 0).collect();
        assert!(columns.len() > 20, "the card vanished");

        let left = height_at(&frame, columns[2]);
        let right = height_at(&frame, columns[columns.len() - 3]);
        assert!(
            left.abs_diff(right) > 20,
            "no foreshortening: {left} against {right}"
        );
    });
}

/// Turning it the other way foreshortens the other side.
#[test]
fn the_foreshortening_follows_the_rotation() {
    with_exporter(|exporter| {
        let turned = |angle: f64| {
            let (mut scene, id) = document();
            scene.update_object(id, |o| {
                o.spatial = Spatial {
                    rotation_y: angle,
                    ..Default::default()
                }
            });
            scene
        };

        let one = render(exporter, &turned(0.8));
        let other = render(exporter, &turned(-0.8));

        let lean = |frame: &Frame| {
            let columns: Vec<u32> =
                (0..frame.width).filter(|x| height_at(frame, *x) > 0).collect();
            let left = height_at(frame, columns[2]) as i64;
            let right = height_at(frame, columns[columns.len() - 3]) as i64;
            (left - right).signum()
        };

        assert_ne!(
            lean(&one),
            lean(&other),
            "reversing the rotation should reverse which side is nearer"
        );
    });
}

/// About the horizontal instead, the top and bottom differ in width.
#[test]
fn tipping_a_card_foreshortens_its_top_and_bottom() {
    with_exporter(|exporter| {
        let (mut scene, id) = document();
        scene.update_object(id, |o| {
            o.spatial = Spatial {
                rotation_x: 0.9,
                ..Default::default()
            }
        });

        let frame = render(exporter, &scene);
        let rows: Vec<u32> = (0..frame.height).filter(|y| width_at(&frame, *y) > 0).collect();
        assert!(rows.len() > 20, "the card vanished");

        let top = width_at(&frame, rows[2]);
        let bottom = width_at(&frame, rows[rows.len() - 3]);
        assert!(
            top.abs_diff(bottom) > 20,
            "no foreshortening: {top} against {bottom}"
        );
    });
}

/// Pushing an object back along its own Z draws it smaller — which is what
/// separates the cards of a tree without moving them on the layer.
#[test]
fn pushing_an_object_back_draws_it_smaller() {
    with_exporter(|exporter| {
        let mut painted = |z: f64| {
            let (mut scene, id) = document();
            scene.update_object(id, |o| {
                o.spatial = Spatial {
                    z,
                    ..Default::default()
                }
            });
            let frame = render(exporter, &scene);
            (0..frame.height).map(|y| width_at(&frame, y) as usize).sum::<usize>()
        };

        let (far, flat, near) = (painted(600.0), painted(0.0), painted(-300.0));
        assert!(far < flat, "further should be smaller: {far} against {flat}");
        assert!(near > flat, "nearer should be bigger: {near} against {flat}");
    });
}

/// **The case this was built for.** Three cards at different angles, and a
/// camera that moves: the picture has to change in a way a flat drawing's
/// could not — the cards turn past each other rather than sliding as one.
#[test]
fn a_tree_of_turned_cards_changes_shape_as_the_camera_moves() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;
        let layer = scene.add_layer("Tree", LayerKind::Normal);

        // Three overlapping cards, each facing its own way — a trunk seen from
        // three sides.
        let mut cards = Vec::new();
        for (index, (x, angle)) in [(220.0, -0.7), (255.0, 0.0), (290.0, 0.7)]
            .into_iter()
            .enumerate()
        {
            let id = scene
                .add_shape(
                    layer,
                    ShapeData::filled(
                        Rect::new(x, 60.0, x + 40.0, 340.0).to_path(1e-9),
                        Color::from_rgb8(0x30, 0x40 + index as u8 * 0x20, 0xC0),
                    ),
                )
                .expect("a card");
            scene.update_object(id, |o| {
                o.spatial = Spatial {
                    rotation_y: angle,
                    ..Default::default()
                }
            });
            cards.push(id);
        }

        scene.camera_mut().enabled = true;

        // The same tree, from two camera positions.
        let shot = |exporter: &mut Exporter, scene: &mut Scene, yaw: f64| {
            scene.camera_mut().set_key(CameraKey {
                yaw,
                ..CameraKey::new(0, Point::new(275.0, 200.0))
            });
            render(exporter, scene)
        };

        let straight = shot(exporter, &mut scene, 0.0);
        let round = shot(exporter, &mut scene, 0.45);

        assert_ne!(
            straight.pixels, round.pixels,
            "the camera move did nothing at all"
        );

        // Each card's own share of the frame changes, which is the thing a
        // flat drawing cannot do: it would only slide.
        let ink = |frame: &Frame| -> usize {
            (0..frame.height)
                .map(|y| width_at(frame, y) as usize)
                .sum()
        };
        let (a, b) = (ink(&straight), ink(&round));
        assert!(
            a.abs_diff(b) * 20 > a,
            "the cards should turn, not merely slide: {a} against {b}"
        );
    });
}
