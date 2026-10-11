//! **A light has a place in the layer stack.**
//!
//! Dragged between two layers in the timeline or the Layers panel, a light
//! lights the layer under it and everything below, and nothing above — the
//! adjustment-layer rule. At the top, where every light starts, it lights the
//! whole picture exactly as before.
//!
//! Read back as pixels from the exporter, so this is what the film shows.

use buzz_doc::serial::DocumentDto;
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerId, LayerKind, LightId, LightKind, Scene, ShapeData};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping light-in-the-stack test: no usable GPU ({e})"),
    }
}

/// Two grey blocks side by side, one per layer, and one sun — shadows off,
/// so each block's colour is its own lighting and nothing cast onto it.
fn stage() -> (Scene, LayerId, LayerId, LightId) {
    let mut scene = Scene::default();
    scene.stage_mut().size = buzz_geom::Size::new(400.0, 200.0);
    scene.stage_mut().background = Color::WHITE;
    let grey = Color::from_rgb8(0x90, 0x90, 0x90);
    let bottom = scene.add_stage_layer("Bottom", LayerKind::Normal);
    let top = scene.add_stage_layer("Top", LayerKind::Normal);
    scene.add_shape(
        top,
        ShapeData::filled(Rect::new(20.0, 40.0, 160.0, 160.0).to_path(1e-9), grey),
    );
    scene.add_shape(
        bottom,
        ShapeData::filled(Rect::new(240.0, 40.0, 380.0, 160.0).to_path(1e-9), grey),
    );
    let light = scene.add_light(LightKind::Sun {
        azimuth: 0.4,
        elevation: 0.5,
    });
    scene.lights_mut().get_mut(light).unwrap().shadows = false;
    // Whichever way `add_stage_layer` stacks, the test names the rows by
    // where they are, not by the order they were made in.
    let front = scene.stage_layer_index(top).unwrap() < scene.stage_layer_index(bottom).unwrap();
    let (upper, lower) = if front { (top, bottom) } else { (bottom, top) };
    (scene, upper, lower, light)
}

/// The two blocks' centres, `(upper layer's block, lower layer's block)`.
fn blocks(scene: &Scene, upper: LayerId, frame: &Frame) -> ([u8; 4], [u8; 4]) {
    let sx = frame.width as f64 / 400.0;
    let at = |x: f64, y: f64| frame.pixel((x * sx) as u32, (y * sx) as u32);
    let (left, right) = (at(90.0, 100.0), at(310.0, 100.0));
    // The "Top" layer holds the left block.
    let top_is_upper = scene.stage_layers().get(upper).unwrap().name == "Top";
    if top_is_upper { (left, right) } else { (right, left) }
}

fn render(exporter: &mut Exporter, scene: &Scene) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("a frame")
}

fn differs(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).any(|(x, y)| x.abs_diff(y) > 3)
}

#[test]
fn a_light_between_layers_lights_only_what_is_below_it() {
    with_exporter(|exporter| {
        let (mut scene, upper, lower, light) = stage();

        let mut unlit = scene.clone();
        unlit.lights_mut().enabled = false;
        let (upper_unlit, lower_unlit) = blocks(&scene, upper, &render(exporter, &unlit));

        // At the top — where it starts — the sun lights both blocks.
        assert_eq!(scene.light_slot(light), None);
        let (upper_top, lower_top) = blocks(&scene, upper, &render(exporter, &scene));
        assert!(
            differs(upper_top, upper_unlit) && differs(lower_top, lower_unlit),
            "the control: a sun at the top lights both blocks \
             (upper {upper_top:?} vs {upper_unlit:?}, lower {lower_top:?} vs {lower_unlit:?})"
        );

        // Dragged down to sit above the lower layer: the upper block is back
        // to unlit, and the lower one is lit exactly as before.
        assert!(scene.place_light(light, Some(lower)));
        assert_eq!(scene.light_slot(light), Some(lower));
        let (upper_mid, lower_mid) = blocks(&scene, upper, &render(exporter, &scene));
        assert!(
            !differs(upper_mid, upper_unlit),
            "a layer above the light is lit by it: {upper_mid:?} against unlit {upper_unlit:?}"
        );
        assert!(
            !differs(lower_mid, lower_top),
            "the layer under the light lost it: {lower_mid:?} against {lower_top:?}"
        );

        // Placed above the upper layer — the top layer in the stack — it lights
        // both again.
        assert!(scene.place_light(light, Some(upper)));
        let (upper_again, _) = blocks(&scene, upper, &render(exporter, &scene));
        assert!(!differs(upper_again, upper_top));
    });
}

#[test]
fn a_light_keeps_its_place_through_a_save() {
    let (mut scene, _, lower, light) = stage();
    scene.place_light(light, Some(lower));
    let loaded = DocumentDto::from_scene(&scene).to_scene().expect("round trip");
    assert_eq!(loaded.light_slot(light), Some(lower));
}

#[test]
fn deleting_the_layer_under_a_light_returns_it_to_the_top() {
    let (mut scene, _, lower, light) = stage();
    scene.place_light(light, Some(lower));
    scene.remove_layer(lower);
    assert_eq!(scene.light_slot(light), None, "a dangling anchor reads as the top");
    // Moving it to where it already reads as being is not a change.
    assert!(!scene.place_light(light, None));
}
