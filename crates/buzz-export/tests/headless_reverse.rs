//! **A turnaround shows the back, on the real GPU.**
//!
//! An object with a red front and a blue reverse: facing the camera it renders
//! red, turned a half-turn it renders blue — the whole point of the feature is a
//! *different drawing* when the object faces away, not the front mirrored. The
//! projection already does the perspective; this proves the swap reaches pixels.
//!
//! Skips with no GPU, like every other headless test here.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Object, ObjectId, Scene, ShapeData};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping reverse test: no usable GPU ({e})"),
    }
}

/// A dark stage with one big square centred on it: red front, blue back,
/// yawed by `rotation_y`.
fn turnaround(rotation_y: f64) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
    let layer = scene.add_layer("Turn", LayerKind::Normal);

    let square = || Rect::new(-120.0, -120.0, 120.0, 120.0).to_path(1e-9);
    let mut front = Object::shape(
        ObjectId(1),
        ShapeData::filled(square(), Color::from_rgb8(0xE0, 0x20, 0x20)),
    );
    front.transform = Affine::translate((275.0, 200.0));
    front.spatial.rotation_y = rotation_y;
    front.turnaround.set(
        std::f64::consts::PI,
        Arc::new(Object::shape(
            ObjectId(2),
            ShapeData::filled(square(), Color::from_rgb8(0x20, 0x40, 0xE0)),
        )),
    );
    scene.add_object(layer, front).expect("the object on a layer");
    scene
}

/// Count strongly-red and strongly-blue pixels in the frame.
fn colour_tally(frame: &Frame) -> (usize, usize) {
    let (mut red, mut blue) = (0, 0);
    for px in frame.pixels.chunks(4) {
        let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
        if r > 120 && r > g + 40 && r > b + 40 {
            red += 1;
        } else if b > 120 && b > r + 40 && b > g + 40 {
            blue += 1;
        }
    }
    (red, blue)
}

#[test]
fn a_facing_object_shows_its_front_and_a_turned_one_its_back() {
    with_exporter(|exporter| {
        let facing = turnaround(0.0);
        let settings = ExportSettings::for_stage(&facing);
        let (red_front, blue_front) = colour_tally(&exporter.render(&facing, 0, &settings).unwrap());
        assert!(
            red_front > 2000 && red_front > blue_front,
            "facing the camera should be red (front): {red_front} red, {blue_front} blue"
        );

        // A half-turn: the object faces away, so the blue back shows instead.
        let turned = turnaround(std::f64::consts::PI);
        let settings = ExportSettings::for_stage(&turned);
        let (red_back, blue_back) = colour_tally(&exporter.render(&turned, 0, &settings).unwrap());
        assert!(
            blue_back > 2000 && blue_back > red_back,
            "turned around should be blue (the back): {red_back} red, {blue_back} blue"
        );
    });
}

// ---------------------------------------------------------------------------
// A whole turnaround: more than two ways round, and a camera that can see them
// ---------------------------------------------------------------------------

/// The same stage, but the character has a **profile** as well as a back, and
/// the camera can be swung round instead of the character being turned.
fn multi_angle(rotation_y: f64, camera_yaw: f64) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
    let layer = scene.add_layer("Turn", LayerKind::Normal);

    let square = || Rect::new(-120.0, -120.0, 120.0, 120.0).to_path(1e-9);
    let mut front = Object::shape(
        ObjectId(1),
        ShapeData::filled(square(), Color::from_rgb8(0xE0, 0x20, 0x20)),
    );
    front.transform = Affine::translate((275.0, 200.0));
    front.spatial.rotation_y = rotation_y;
    // Blue at the back, green in profile.
    front.turnaround.set(
        std::f64::consts::PI,
        Arc::new(Object::shape(
            ObjectId(2),
            ShapeData::filled(square(), Color::from_rgb8(0x20, 0x40, 0xE0)),
        )),
    );
    front.turnaround.set(
        std::f64::consts::FRAC_PI_2,
        Arc::new(Object::shape(
            ObjectId(3),
            ShapeData::filled(square(), Color::from_rgb8(0x20, 0xE0, 0x40)),
        )),
    );
    scene.add_object(layer, front).expect("the object on a layer");

    if camera_yaw != 0.0 {
        let cam = scene.camera_mut();
        cam.enabled = true;
        let mut key = buzz_scene::CameraKey::new(0, buzz_geom::Point::new(275.0, 200.0));
        key.yaw = camera_yaw;
        cam.set_key(key);
    }
    scene
}

/// How much green is in the frame.
fn green_count(frame: &Frame) -> usize {
    frame
        .pixels
        .chunks(4)
        .filter(|px| px[1] > 120 && px[0] < 100 && px[2] < 100)
        .count()
}

/// **A profile is visible at all.** Turned exactly side-on, the character's own
/// plane is edge-on and has no width to draw in — so a turnaround that could
/// only foreshorten would show nothing. Swapping to the profile drawing and
/// keeping only the leftover turn is what puts pixels on the screen.
#[test]
fn a_side_on_character_shows_its_profile() {
    with_exporter(|exporter| {
        let scene = multi_angle(std::f64::consts::FRAC_PI_2, 0.0);
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("side on");

        let green = green_count(&frame);
        assert!(
            green > 1000,
            "the profile drawing should fill much of the frame, got {green} green pixels"
        );
        let (red, blue) = colour_tally(&frame);
        assert!(
            green > red && green > blue,
            "and it should be the profile, not the front or back: r{red} g{green} b{blue}"
        );
    });
}

/// **The camera can walk round the character.** Nothing on the stage turns
/// here — the shot does — and the character must show the side the camera has
/// moved to. Facing that only reads the object's own yaw cannot do this.
#[test]
fn swinging_the_camera_round_shows_the_other_side() {
    with_exporter(|exporter| {
        // The character faces straight ahead throughout.
        let straight_on = multi_angle(0.0, 0.0);
        let settings = ExportSettings::for_stage(&straight_on);
        let (red_before, _) = colour_tally(
            &exporter
                .render(&straight_on, 0, &settings)
                .expect("straight on"),
        );
        assert!(red_before > 1000, "it starts as the red front");

        // Now swing the camera most of the way round to its back.
        let from_behind = multi_angle(0.0, -std::f64::consts::PI + 0.2);
        let frame = exporter
            .render(&from_behind, 0, &settings)
            .expect("from behind");
        let (red_after, blue_after) = colour_tally(&frame);
        assert!(
            blue_after > red_after,
            "a camera behind the character should see its back: r{red_after} b{blue_after}"
        );
    });
}
