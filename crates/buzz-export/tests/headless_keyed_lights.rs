//! A keyframed light lights the frame it is drawn at, not one fixed state.
//!
//! Wave 9a's promise on the real GPU: a sun animated from low on the horizon to
//! high overhead fills the artwork more as it climbs, so the same document
//! rendered at two frames comes back at two brightnesses. A static rig is
//! unchanged frame to frame, which is the control. Skips with no GPU.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{
    Light, LightId, LightKey, LightKind, LightTrack, LayerKind, Scene, ShapeData,
};
use peniko::Color;

const ART: Color = Color::from_rgb8(0x90, 0x90, 0x90);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping keyed-light test: no usable GPU ({e})"),
    }
}

/// A grey stage with a grey square, lit by one sun that climbs from frame 0 to
/// frame 20.
fn document() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x30, 0x30, 0x30);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(150.0, 100.0, 400.0, 300.0).to_path(1e-9), ART),
    );
    // Hold the artwork past frame 20, so the frames the lights are keyed at
    // actually have something on them to light.
    scene.update_layer(layer, |l| {
        l.frames.insert_frame(25);
    });

    let mut sun = Light::new(
        LightId(1),
        "Key",
        LightKind::Sun {
            azimuth: 0.0,
            elevation: 0.15,
        },
    );
    let mut track = LightTrack::new();
    track.enabled = true;
    // Low on the horizon at frame 0 …
    track.set_key(LightKey {
        frame: 0,
        color: Color::WHITE,
        intensity: 1.0,
        softness: 0.35,
        standing_height: 40.0,
        shadow_strength: 0.45,
        kind: LightKind::Sun {
            azimuth: 0.0,
            elevation: 0.15,
        },
    });
    // … overhead at frame 20.
    track.set_key(LightKey {
        frame: 20,
        color: Color::WHITE,
        intensity: 1.0,
        softness: 0.35,
        standing_height: 40.0,
        shadow_strength: 0.45,
        kind: LightKind::Sun {
            azimuth: 0.0,
            elevation: 1.4,
        },
    });
    sun.track = Some(track);

    let rig = scene.lights_mut();
    rig.enabled = true;
    rig.lights.push(sun);
    rig
        .lights
        .push(Light::new(LightId(2), "Sky", LightKind::sky()));
    scene
}

fn mean_luma(frame: &Frame) -> f64 {
    let mut sum = 0.0;
    let n = (frame.width * frame.height) as f64;
    for px in frame.pixels.chunks(4) {
        sum += 0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64;
    }
    sum / n
}

#[test]
fn a_climbing_sun_brightens_the_frame() {
    with_exporter(|exporter| {
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);

        let low = exporter.render(&scene, 0, &settings).expect("frame 0");
        let high = exporter.render(&scene, 20, &settings).expect("frame 20");

        let low_l = mean_luma(&low);
        let high_l = mean_luma(&high);
        assert!(
            high_l > low_l + 1.0,
            "the overhead sun should light the frame more than the low one: \
             low {low_l:.1}, high {high_l:.1}"
        );
    });
}

#[test]
fn a_static_rig_is_unchanged_between_frames() {
    with_exporter(|exporter| {
        // Same document, but with the track switched off: the light holds one
        // state, so the two frames match.
        let mut scene = document();
        if let Some(track) = scene.lights_mut().lights[0].track.as_mut() {
            track.enabled = false;
        }
        let settings = ExportSettings::for_stage(&scene);

        let a = exporter.render(&scene, 0, &settings).expect("frame 0");
        let b = exporter.render(&scene, 20, &settings).expect("frame 20");
        let diff = (mean_luma(&a) - mean_luma(&b)).abs();
        assert!(
            diff < 0.01,
            "a static light must not change with the frame (diff {diff})"
        );
    });
}
