//! Prove the filters on the real GPU.
//!
//! A filter is a claim about pixels — "there is a shadow down and to the
//! right", "the edge is soft now" — and nothing but reading the pixels back
//! can check it. These render frames through the same walk the window and the
//! exporter use, and look at what came out.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{
    Blend, ColorAdjust, Filter, FilterKind, LayerKind, ObjectId, Quality, Scene, ShapeData,
};
use peniko::Color;

const ART: Color = Color::from_rgb8(0x30, 0x60, 0xC0);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping filter test: no usable GPU ({e})");
            None
        }
    });
    match shared {
        Some(mutex) => test(&mut mutex.lock().unwrap_or_else(|e| e.into_inner())),
        None => eprintln!("skipping: no usable GPU"),
    }
}

/// A white stage with one blue square in the middle, and room round it.
fn document() -> (Scene, ObjectId) {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    let id = scene
        .add_shape(
            layer,
            ShapeData::filled(Rect::new(200.0, 150.0, 350.0, 300.0).to_path(1e-9), ART),
        )
        .expect("the square");
    (scene, id)
}

fn render(exporter: &mut Exporter, scene: &Scene) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("render")
}

fn luma(pixel: [u8; 4]) -> f32 {
    0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32
}

fn with_filter(kind: FilterKind) -> Scene {
    let (mut scene, id) = document();
    scene.update_object(id, |o| o.filters = vec![Filter::new(kind)]);
    scene
}

/// The promise every existing document depends on: no filters, no change.
#[test]
fn a_document_without_filters_renders_exactly_as_before() {
    with_exporter(|exporter| {
        let (scene, id) = document();
        let before = render(exporter, &scene);

        // A filter added and removed again leaves no trace.
        let mut touched = scene.clone();
        touched.update_object(id, |o| o.filters = vec![Filter::new(FilterKind::glow())]);
        touched.update_object(id, |o| o.filters.clear());

        assert_eq!(
            before.pixels,
            render(exporter, &touched).pixels,
            "an unfiltered document must be pixel-identical"
        );
    });
}

/// A drop shadow lands on the far side of the artwork, and nowhere else.
#[test]
fn a_drop_shadow_falls_on_the_side_the_light_says() {
    with_exporter(|exporter| {
        // 45°, distance 20: down and to the right of the square.
        let scene = with_filter(FilterKind::DropShadow {
            x: 8.0,
            y: 8.0,
            strength: 1.0,
            angle: std::f64::consts::FRAC_PI_4,
            distance: 20.0,
            color: Color::BLACK,
            inner: false,
            knockout: false,
            hide_object: false,
            quality: Quality::Medium,
        });
        let frame = render(exporter, &scene);

        let below_right = luma(frame.pixel(360, 310));
        let above_left = luma(frame.pixel(190, 140));
        assert!(
            below_right < 220.0,
            "no shadow below and right of the square: {below_right}"
        );
        assert!(
            above_left > 245.0,
            "the far corner should be untouched: {above_left}"
        );
    });
}

/// Turning the light round turns the shadow round.
#[test]
fn the_shadow_swings_with_its_angle() {
    with_exporter(|exporter| {
        let at = |angle: f64| {
            with_filter(FilterKind::DropShadow {
                x: 6.0,
                y: 6.0,
                strength: 1.0,
                angle,
                distance: 25.0,
                color: Color::BLACK,
                inner: false,
                knockout: false,
                hide_object: false,
                quality: Quality::Low,
            })
        };

        let east = render(exporter, &at(0.0));
        let west = render(exporter, &at(std::f64::consts::PI));

        assert!(
            luma(east.pixel(365, 225)) < luma(east.pixel(185, 225)),
            "a shadow to the east should darken the right"
        );
        assert!(
            luma(west.pixel(185, 225)) < luma(west.pixel(365, 225)),
            "and to the west, the left"
        );
    });
}

/// A glow puts its colour outside the artwork, all the way round.
#[test]
fn a_glow_surrounds_the_artwork() {
    with_exporter(|exporter| {
        let scene = with_filter(FilterKind::Glow {
            x: 16.0,
            y: 16.0,
            strength: 1.0,
            color: Color::from_rgb8(0xFF, 0x00, 0x00),
            inner: false,
            knockout: false,
            quality: Quality::Medium,
        });
        let frame = render(exporter, &scene);

        for (x, y, side) in [
            (195, 225, "left"),
            (355, 225, "right"),
            (275, 145, "top"),
            (275, 305, "bottom"),
        ] {
            let pixel = frame.pixel(x, y);
            assert!(
                pixel[0] > pixel[2] + 20,
                "no red glow on the {side}: {pixel:?}"
            );
        }
    });
}

/// A glow fades: near the edge it is strong, further out it is faint. That
/// gradient is the whole difference between a glow and an outline.
#[test]
fn a_glow_fades_with_distance() {
    with_exporter(|exporter| {
        let scene = with_filter(FilterKind::Glow {
            x: 30.0,
            y: 30.0,
            strength: 1.0,
            color: Color::from_rgb8(0xFF, 0x00, 0x00),
            inner: false,
            knockout: false,
            quality: Quality::High,
        });
        let frame = render(exporter, &scene);

        let redness = |x: u32| {
            let p = frame.pixel(x, 225);
            p[0] as i32 - p[2] as i32
        };
        let near = redness(355);
        let far = redness(375);
        assert!(
            near > far,
            "the glow should fade outwards: {near} at the edge, {far} further out"
        );
        assert!(far >= 0, "and never invert: {far}");
    });
}

/// Blur softens the edge: the hard step from artwork to background becomes a
/// ramp, so a pixel just outside the shape is no longer pure background.
#[test]
fn a_blur_softens_the_edge() {
    with_exporter(|exporter| {
        let (sharp, _) = document();
        let sharp_frame = render(exporter, &sharp);
        assert!(
            luma(sharp_frame.pixel(355, 225)) > 250.0,
            "the unfiltered square should have a hard edge"
        );

        let scene = with_filter(FilterKind::Blur {
            x: 20.0,
            y: 20.0,
            quality: Quality::Medium,
        });
        let frame = render(exporter, &scene);

        let outside = luma(frame.pixel(355, 225));
        assert!(
            outside < 245.0,
            "the blur should spread past the edge: {outside}"
        );
        // And it fades: further out is lighter than nearer in.
        assert!(
            luma(frame.pixel(365, 225)) > outside,
            "the blur should fade outwards"
        );
    });
}

/// Adjust Color is arithmetic, not geometry: the fill really is brighter.
#[test]
fn adjust_colour_changes_the_artwork_itself() {
    with_exporter(|exporter| {
        let (plain, _) = document();
        let before = plain_pixel(exporter, &plain);

        let scene = with_filter(FilterKind::Adjust(ColorAdjust {
            brightness: 40.0,
            ..Default::default()
        }));
        let after = plain_pixel(exporter, &scene);

        assert!(
            after[0] > before[0] && after[1] > before[1] && after[2] > before[2],
            "brightness should lift every channel: {before:?} -> {after:?}"
        );
    });
}

fn plain_pixel(exporter: &mut Exporter, scene: &Scene) -> [u8; 4] {
    render(exporter, scene).pixel(275, 225)
}

/// Knockout keeps the effect and drops the artwork — which is what makes it
/// useful, and what makes it obvious when it is wrong.
#[test]
fn knockout_leaves_the_shadow_without_the_artwork() {
    with_exporter(|exporter| {
        let scene = with_filter(FilterKind::DropShadow {
            x: 6.0,
            y: 6.0,
            strength: 1.0,
            angle: std::f64::consts::FRAC_PI_4,
            distance: 30.0,
            color: Color::BLACK,
            inner: false,
            knockout: true,
            hide_object: false,
            quality: Quality::Low,
        });
        let frame = render(exporter, &scene);

        let middle = frame.pixel(210, 160);
        assert!(
            middle[2] < 200 || luma(middle) > 200.0,
            "the artwork should be gone: {middle:?}"
        );
        assert!(
            luma(frame.pixel(360, 310)) < 200.0,
            "but the shadow should still be there"
        );
    });
}

/// A blend mode changes how an object meets what is behind it.
#[test]
fn multiply_darkens_where_two_shapes_overlap() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;
        let back = scene.add_layer("Back", LayerKind::Normal);
        let front = scene.add_layer("Front", LayerKind::Normal);

        scene.add_shape(
            back,
            ShapeData::filled(
                Rect::new(150.0, 150.0, 300.0, 300.0).to_path(1e-9),
                Color::from_rgb8(0xC0, 0xC0, 0x40),
            ),
        );
        let id = scene
            .add_shape(
                front,
                ShapeData::filled(
                    Rect::new(220.0, 150.0, 380.0, 300.0).to_path(1e-9),
                    Color::from_rgb8(0x40, 0xC0, 0xC0),
                ),
            )
            .expect("the front square");

        let plain = render(exporter, &scene).pixel(260, 225);
        scene.update_object(id, |o| o.blend = Blend::Multiply);
        let multiplied = render(exporter, &scene).pixel(260, 225);

        assert!(
            luma(multiplied) < luma(plain),
            "multiply should darken the overlap: {plain:?} -> {multiplied:?}"
        );
    });
}

/// A filter on the *layer* treats everything on it as one subject — which
/// Animate cannot do at all.
#[test]
fn a_layer_filter_applies_to_the_whole_layer() {
    with_exporter(|exporter| {
        let (mut scene, _) = document();
        let layer = scene.layers().iter().next().map(|l| l.id).expect("a layer");
        // A second square on the same layer, well away from the first.
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(60.0, 60.0, 140.0, 140.0).to_path(1e-9), ART),
        );

        let before = render(exporter, &scene);
        scene.update_layer(layer, |l| {
            l.filters = vec![Filter::new(FilterKind::Glow {
                x: 14.0,
                y: 14.0,
                strength: 1.0,
                color: Color::from_rgb8(0xFF, 0x00, 0x00),
                inner: false,
                knockout: false,
                quality: Quality::Low,
            })];
        });
        let after = render(exporter, &scene);

        assert_ne!(before.pixels, after.pixels, "the layer filter did nothing");
        for (x, y, which) in [(150, 100, "the small square"), (360, 225, "the big one")] {
            let pixel = after.pixel(x, y);
            assert!(pixel[0] > pixel[2] + 20, "no glow round {which}: {pixel:?}");
        }
    });
}

/// A filter switched off paints nothing — the flag has to reach the renderer,
/// not just the panel.
#[test]
fn a_disabled_filter_is_not_drawn() {
    with_exporter(|exporter| {
        let (plain, id) = document();
        let before = render(exporter, &plain);

        let mut scene = plain.clone();
        scene.update_object(id, |o| {
            o.filters = vec![Filter {
                kind: FilterKind::glow(),
                enabled: false,
            }];
        });

        assert_eq!(before.pixels, render(exporter, &scene).pixels);
    });
}
