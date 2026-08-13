//! Prove the spatial camera on the real GPU.
//!
//! The claim is that tilting the camera puts artwork in *perspective*: a
//! rectangle becomes a trapezoid, the near edge wider than the far one, and
//! parallel lines converge. Nothing but reading the pixels back can check that
//! — the arithmetic could be right and the renderer could still be handing
//! Vello the affine it always did.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{CameraKey, LayerKind, Scene, ShapeData};
use peniko::Color;

const ART: Color = Color::from_rgb8(0x20, 0x50, 0xC0);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping camera test: no usable GPU ({e})");
            None
        }
    });
    match shared {
        Some(mutex) => test(&mut mutex.lock().unwrap_or_else(|e| e.into_inner())),
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// A white stage with one wide blue band across the middle — a shape whose
/// width can be measured at the top and at the bottom.
fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(100.0, 80.0, 450.0, 320.0).to_path(1e-9), ART),
    );
    scene
}

fn render(exporter: &mut Exporter, scene: &Scene) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("render")
}

/// How wide the artwork is on one scanline, in pixels.
fn width_at(frame: &Frame, y: u32) -> u32 {
    let mut count = 0;
    for x in 0..frame.width {
        let p = frame.pixel(x, y);
        // Blue against a white stage. Widened to `i32` deliberately: a white
        // pixel is 255 and `p[0] + 20` overflows a `u8`.
        if p[2] as i32 > p[0] as i32 + 20 {
            count += 1;
        }
    }
    count
}

/// The promise every document depends on: no tilt, no change.
#[test]
fn an_untilted_camera_renders_exactly_as_before() {
    with_exporter(|exporter| {
        let plain = document();
        let before = render(exporter, &plain);

        // A camera enabled and keyed, but never tilted.
        let mut keyed = document();
        keyed.camera_mut().enabled = true;
        keyed
            .camera_mut()
            .set_key(CameraKey::new(0, Point::new(275.0, 200.0)));
        let after = render(exporter, &keyed);

        assert_eq!(
            before.pixels, after.pixels,
            "an untilted camera must be pixel-identical"
        );
    });
}

/// The whole point: pitch the camera and the rectangle becomes a trapezoid.
#[test]
fn pitching_the_camera_puts_the_artwork_in_perspective() {
    with_exporter(|exporter| {
        let mut scene = document();
        scene.camera_mut().enabled = true;
        scene.camera_mut().set_key(CameraKey {
            pitch: 0.55,
            ..CameraKey::new(0, Point::new(275.0, 200.0))
        });

        let frame = render(exporter, &scene);
        let near_top = width_at(&frame, 120);
        let near_bottom = width_at(&frame, 300);

        assert!(
            near_top > 0 && near_bottom > 0,
            "the artwork vanished: {near_top} / {near_bottom}"
        );
        // One edge must be *measurably* wider than the other: that difference
        // is the perspective, and an affine cannot produce it.
        let (wide, narrow) = if near_top > near_bottom {
            (near_top, near_bottom)
        } else {
            (near_bottom, near_top)
        };
        assert!(
            wide > narrow + 20,
            "no convergence: {wide} against {narrow}"
        );
    });
}

/// Turning the pitch the other way turns the trapezoid the other way.
#[test]
fn the_perspective_follows_the_tilt() {
    with_exporter(|exporter| {
        let shot = |pitch: f64| {
            let mut scene = document();
            scene.camera_mut().enabled = true;
            scene.camera_mut().set_key(CameraKey {
                pitch,
                ..CameraKey::new(0, Point::new(275.0, 200.0))
            });
            scene
        };

        let down = render(exporter, &shot(0.5));
        let up = render(exporter, &shot(-0.5));

        let down_top = width_at(&down, 130) as i64;
        let down_bottom = width_at(&down, 290) as i64;
        let up_top = width_at(&up, 130) as i64;
        let up_bottom = width_at(&up, 290) as i64;

        assert!(
            (down_top - down_bottom).signum() != (up_top - up_bottom).signum(),
            "reversing the tilt should reverse which edge is wider: \
             down {down_top}/{down_bottom}, up {up_top}/{up_bottom}"
        );
    });
}

/// Yaw does it about the other axis: the left and right edges differ in
/// *height* rather than the top and bottom differing in width.
#[test]
fn yaw_converges_the_other_way() {
    with_exporter(|exporter| {
        let mut scene = document();
        scene.camera_mut().enabled = true;
        scene.camera_mut().set_key(CameraKey {
            yaw: 0.55,
            ..CameraKey::new(0, Point::new(275.0, 200.0))
        });
        let frame = render(exporter, &scene);

        let height_at = |x: u32| {
            let mut count = 0;
            for y in 0..frame.height {
                let p = frame.pixel(x, y);
                if p[2] as i32 > p[0] as i32 + 20 {
                    count += 1;
                }
            }
            count
        };

        let left = height_at(150);
        let right = height_at(400);
        assert!(left > 0 && right > 0, "the artwork vanished");
        let (tall, short) = if left > right { (left, right) } else { (right, left) };
        assert!(
            tall > short + 15,
            "no convergence across the frame: {tall} against {short}"
        );
    });
}

/// Tilt and layer depth compose: a layer further away is still drawn smaller,
/// and still in perspective.
#[test]
fn a_far_layer_is_smaller_and_still_in_perspective() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;
        let near = scene.add_layer("Near", LayerKind::Normal);
        scene.add_shape(
            near,
            ShapeData::filled(Rect::new(100.0, 80.0, 450.0, 320.0).to_path(1e-9), ART),
        );
        scene.camera_mut().enabled = true;
        scene.camera_mut().set_key(CameraKey {
            pitch: 0.45,
            ..CameraKey::new(0, Point::new(275.0, 200.0))
        });

        let close = render(exporter, &scene);
        let close_width = width_at(&close, 200);

        // The same artwork, pushed into the distance.
        scene.update_layer(near, |l| l.depth = 1500.0);
        let far = render(exporter, &scene);
        let far_width = width_at(&far, 200);

        assert!(far_width > 0, "the far layer vanished");
        assert!(
            far_width < close_width,
            "depth should still shrink it: {far_width} against {close_width}"
        );
    });
}

/// A camera tilted to the limit still produces a picture rather than a blank
/// frame or a smear — the clamp and the horizon clip both have to hold.
#[test]
fn a_camera_at_the_limit_still_draws_something() {
    with_exporter(|exporter| {
        let mut scene = document();
        scene.camera_mut().enabled = true;
        scene.camera_mut().set_key(CameraKey {
            pitch: 5.0,
            yaw: -5.0,
            ..CameraKey::new(0, Point::new(275.0, 200.0))
        });

        let frame = render(exporter, &scene);
        let painted: usize = (0..frame.height)
            .map(|y| width_at(&frame, y) as usize)
            .sum();
        assert!(painted > 0, "a clamped camera drew nothing at all");
    });
}
