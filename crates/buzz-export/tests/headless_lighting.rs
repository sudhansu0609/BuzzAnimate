//! Prove the lights on the real GPU: an unlit document is untouched, a sun
//! shades and casts, and moving the sun moves what it does.
//!
//! Lighting is the one feature where "it compiles and the numbers look right"
//! proves nothing at all. What matters is whether the picture changes, in the
//! direction the light points — and these read the pixels back to say so.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, LightKind, Scene, ShapeData};
use peniko::Color;

const ART: Color = Color::from_rgb8(0xC0, 0xC0, 0xC0);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping lighting test: no usable GPU ({e})");
            None
        }
    });
    match shared {
        Some(mutex) => test(&mut mutex.lock().unwrap_or_else(|e| e.into_inner())),
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// A white stage with one grey square in the middle, and room around it for a
/// shadow to fall into.
fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(200.0, 150.0, 350.0, 300.0).to_path(1e-9), ART),
    );
    scene
}

fn render(exporter: &mut Exporter, scene: &Scene) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("render")
}

fn luma(pixel: [u8; 4]) -> f32 {
    0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32
}

/// The promise every existing document depends on: no lights, no change.
#[test]
fn a_document_without_lights_renders_exactly_as_before() {
    with_exporter(|exporter| {
        let scene = document();
        let before = render(exporter, &scene);

        // A rig that exists but holds nothing must also change nothing.
        let mut with_rig = scene.clone();
        let _ = with_rig.lights_mut();
        let after = render(exporter, &with_rig);

        assert_eq!(
            before.pixels, after.pixels,
            "an unlit document must be pixel-identical"
        );
    });
}

/// Switching a sun on has to *do* something — and the artwork must still be
/// recognisable rather than going black.
#[test]
fn adding_a_sun_changes_the_picture_without_destroying_it() {
    with_exporter(|exporter| {
        let plain = document();
        let unlit = render(exporter, &plain);

        let mut lit_scene = document();
        lit_scene.add_light(LightKind::sun());
        let lit = render(exporter, &lit_scene);

        assert_ne!(unlit.pixels, lit.pixels, "the sun did nothing at all");

        let middle = lit.pixel(275, 225);
        assert!(
            luma(middle) > 40.0,
            "the artwork went almost black: {middle:?}"
        );
    });
}

/// The whole point: the shaded side is opposite the light, and swapping the
/// sun's side swaps which edge is dark.
#[test]
fn the_shaded_side_follows_the_sun() {
    with_exporter(|exporter| {
        let mut from_left = document();
        // Azimuth pi: the sun lies towards -x, so the left edge is lit.
        let id = from_left.add_light(LightKind::Sun {
            azimuth: std::f64::consts::PI,
            elevation: 0.5,
        });
        from_left.lights_mut().get_mut(id).expect("the sun").shadows = false;

        let mut from_right = from_left.clone();
        from_right.lights_mut().get_mut(id).expect("the sun").kind = LightKind::Sun {
            azimuth: 0.0,
            elevation: 0.5,
        };

        let left_lit = render(exporter, &from_left);
        let right_lit = render(exporter, &from_right);

        // Just inside each edge of the square, at its middle height.
        let sample = |frame: &Frame, x: u32| luma(frame.pixel(x, 225));
        let (left_edge, right_edge) = (215, 335);

        assert!(
            sample(&left_lit, left_edge) > sample(&left_lit, right_edge) + 4.0,
            "lit from the left, the left edge should be brighter: {} vs {}",
            sample(&left_lit, left_edge),
            sample(&left_lit, right_edge)
        );
        assert!(
            sample(&right_lit, right_edge) > sample(&right_lit, left_edge) + 4.0,
            "lit from the right, the right edge should be brighter: {} vs {}",
            sample(&right_lit, right_edge),
            sample(&right_lit, left_edge)
        );
    });
}

/// A cast shadow lands on the background, on the far side from the sun, and
/// swings round when the sun does.
#[test]
fn the_cast_shadow_falls_away_from_the_sun_and_swings_with_it() {
    with_exporter(|exporter| {
        let mut scene = document();
        let id = scene.add_light(LightKind::Sun {
            // Towards +x and low: the shadow falls to the left of the square.
            azimuth: 0.0,
            elevation: 0.45,
        });
        {
            let sun = scene.lights_mut().get_mut(id).expect("the sun");
            sun.standing_height = 60.0;
            sun.shadow_strength = 0.8;
        }

        let cast_left = render(exporter, &scene);
        let left_of = luma(cast_left.pixel(170, 225));
        let right_of = luma(cast_left.pixel(380, 225));
        assert!(
            left_of < right_of - 20.0,
            "the shadow should darken the background to the left: {left_of} vs {right_of}"
        );

        // Turn the sun around: the shadow must move to the other side.
        scene.lights_mut().get_mut(id).expect("the sun").kind = LightKind::Sun {
            azimuth: std::f64::consts::PI,
            elevation: 0.45,
        };
        let cast_right = render(exporter, &scene);
        let left_of = luma(cast_right.pixel(170, 225));
        let right_of = luma(cast_right.pixel(380, 225));
        assert!(
            right_of < left_of - 20.0,
            "the shadow should have swung to the right: {left_of} vs {right_of}"
        );
    });
}

/// A lower sun throws a longer shadow — what makes a light feel like a light
/// rather than a drop-shadow filter.
#[test]
fn a_lower_sun_throws_a_longer_shadow() {
    with_exporter(|exporter| {
        let mut reach = |elevation: f64| {
            let mut scene = document();
            let id = scene.add_light(LightKind::Sun {
                azimuth: 0.0,
                elevation,
            });
            {
                let sun = scene.lights_mut().get_mut(id).expect("the sun");
                sun.standing_height = 50.0;
                sun.shadow_strength = 0.9;
            }
            let frame = render(exporter, &scene);

            // How far left of the square the darkening *reaches*: scan
            // inwards from the stage edge and stop at the first dark pixel.
            // Scanning outwards from the square instead measures where the
            // shadow starts, which is the same for every elevation.
            (0..200u32)
                .find(|x| luma(frame.pixel(*x, 225)) < 200.0)
                .unwrap_or(200)
        };

        let high = reach(1.2);
        let low = reach(0.35);
        assert!(
            low < high,
            "a lower sun should reach further left: low starts at x={low}, high at x={high}"
        );
    });
}

/// A warm light makes the artwork warm; a cold one makes it cold.
#[test]
fn the_lights_colour_reaches_the_artwork() {
    with_exporter(|exporter| {
        let mut tint = |colour: Color| {
            let mut scene = document();
            let id = scene.add_light(LightKind::Sun {
                azimuth: 0.0,
                elevation: 1.3,
            });
            {
                let sun = scene.lights_mut().get_mut(id).expect("the sun");
                sun.color = colour;
                sun.intensity = 1.4;
                sun.shadows = false;
            }
            render(exporter, &scene).pixel(275, 225)
        };

        let warm = tint(Color::from_rgb8(0xFF, 0x80, 0x20));
        let cold = tint(Color::from_rgb8(0x20, 0x80, 0xFF));

        assert!(
            warm[0] > cold[0] && cold[2] > warm[2],
            "a warm light should read warmer than a cold one: {warm:?} vs {cold:?}"
        );
    });
}

/// A lamp lights what is near it more than what is far — the difference
/// between a lamp and a sun, in one picture.
#[test]
fn a_lamp_lights_what_is_near_it_more_than_what_is_far() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::from_rgb8(0x20, 0x20, 0x20);
        let layer = scene.add_layer("Art", LayerKind::Normal);
        // Two identical squares: one beside the lamp, one across the stage.
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(40.0, 170.0, 140.0, 270.0).to_path(1e-9), ART),
        );
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(420.0, 170.0, 520.0, 270.0).to_path(1e-9), ART),
        );

        let id = scene.add_light(LightKind::Lamp {
            position: Point::new(60.0, 220.0),
            height: 120.0,
            radius: 180.0,
        });
        {
            let lamp = scene.lights_mut().get_mut(id).expect("the lamp");
            lamp.intensity = 3.0;
            lamp.shadows = false;
        }
        scene.lights_mut().base = Color::from_rgb8(0x18, 0x18, 0x18);

        let frame = render(exporter, &scene);
        let near = luma(frame.pixel(90, 220));
        let far = luma(frame.pixel(470, 220));

        assert!(
            near > far + 15.0,
            "the near square should be brighter than the far one: {near} vs {far}"
        );
    });
}

/// A sky is fill, not direction: it lights everything and casts nothing.
#[test]
fn a_sky_fills_without_casting() {
    with_exporter(|exporter| {
        let mut scene = document();
        scene.add_light(LightKind::sky());
        scene.lights_mut().base = Color::BLACK;

        let frame = render(exporter, &scene);

        assert!(luma(frame.pixel(275, 225)) > 20.0, "the sky should light it");

        let left = luma(frame.pixel(170, 225));
        let right = luma(frame.pixel(380, 225));
        assert!(
            (left - right).abs() < 3.0,
            "a sky must not cast a shadow: {left} vs {right}"
        );
    });
}
