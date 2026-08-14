//! Prove gradients on the real GPU, through the same render path the window
//! uses.
//!
//! A gradient is the first paint in this program that is not one colour, and
//! the thing that can go wrong with it is not "wrong colour" but **wrong
//! place**: the ramp drawn somewhere other than across the shape it fills. The
//! path is pre-transformed into render space on the CPU while the brush is
//! placed by a matrix Vello composes on the GPU, and the two travel by
//! different routes to the same pixel. If they ever disagree, the gradient
//! slides off its artwork — so the assertions below are about *where* each
//! colour lands, not merely that both appear.
//!
//! Skips cleanly when no GPU is available, so it is safe in headless CI.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{
    FillSpec, Gradient, GradientKind, GradientSpread, GradientStop, LayerKind, Scene, ShapeData,
};
use peniko::Color;

const RED: Color = Color::from_rgb8(0xFF, 0x00, 0x00);
const BLUE: Color = Color::from_rgb8(0x00, 0x00, 0xFF);

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping gradient test: no usable GPU ({e})");
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

/// A stage holding one rectangle, filled with `gradient`.
fn staged(gradient: Gradient, area: Rect) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData {
            path: area.to_path(1e-9),
            fill: Some(FillSpec::gradient(gradient)),
            stroke: None,
            blend: buzz_scene::PaintBlend::Normal,
        },
    );
    scene
}

fn render(scene: &Scene, exporter: &mut Exporter) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("render")
}

/// How red a pixel is, minus how blue it is: +1 at pure red, −1 at pure blue.
///
/// Reading the ramp as one number rather than comparing against named colours
/// keeps the assertions about *direction* — which is what a gradient is — and
/// immune to the exact interpolation the GPU does in between.
fn redness(pixel: [u8; 4]) -> f64 {
    (f64::from(pixel[0]) - f64::from(pixel[2])) / 255.0
}

/// The plain claim: red at the left edge, blue at the right, and a monotone
/// run between them.
#[test]
fn a_linear_gradient_runs_across_the_shape_it_fills() {
    with_exporter(|exporter| {
        let area = Rect::new(50.0, 50.0, 450.0, 350.0);
        let scene = staged(Gradient::linear(RED, BLUE, area), area);
        let frame = render(&scene, exporter);

        let y = 200;
        let left = redness(frame.pixel(60, y));
        let mid = redness(frame.pixel(250, y));
        let right = redness(frame.pixel(440, y));

        assert!(left > 0.8, "the left edge should be red, was {left:.3}");
        assert!(
            right < -0.8,
            "the right edge should be blue, was {right:.3}"
        );
        assert!(
            mid.abs() < 0.25,
            "the middle should be halfway, was {mid:.3}"
        );

        // Monotone all the way across: a ramp that doubled back would mean the
        // brush transform had flipped somewhere.
        let mut previous = f64::INFINITY;
        for x in (55..445).step_by(10) {
            let here = redness(frame.pixel(x, y));
            assert!(
                here <= previous + 0.02,
                "the ramp reversed at x={x}: {here:.3} after {previous:.3}"
            );
            previous = here;
        }
    });
}

/// **The assertion this file exists for.** The shape is moved by its own
/// transform after the gradient was fitted to it, so the gradient has to travel
/// with it. If the brush were placed in document space while the path was
/// placed by the object's transform, the ramp would stay behind and the shape
/// would come out one flat colour — which is exactly what a naive
/// implementation does, and it looks like a gradient that "did not work"
/// rather than like a transform bug.
#[test]
fn a_gradient_travels_with_the_object_that_carries_it() {
    with_exporter(|exporter| {
        // Fitted to a rectangle at the origin, then the *object* is moved to
        // the middle of the stage.
        let area = Rect::new(0.0, 0.0, 200.0, 200.0);
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let id = scene.add_shape(
            layer,
            ShapeData {
                path: area.to_path(1e-9),
                fill: Some(FillSpec::gradient(Gradient::linear(RED, BLUE, area))),
                stroke: None,
                blend: buzz_scene::PaintBlend::Normal,
            },
        );
        let id = id.expect("the shape should have been added");
        scene.update_object(id, |o| {
            o.transform = Affine::translate((150.0, 100.0));
        });

        let frame = render(&scene, exporter);

        // The shape now spans x = 150..350 at y = 100..300.
        let y = 200;
        let left = redness(frame.pixel(160, y));
        let right = redness(frame.pixel(340, y));

        assert!(
            left > 0.8,
            "the moved shape's left edge should still be red, was {left:.3}"
        );
        assert!(
            right < -0.8,
            "the moved shape's right edge should still be blue, was {right:.3} \
             — the gradient did not follow its shape"
        );
    });
}

/// A radial gradient is hot in the middle and cold at the rim, in both axes.
#[test]
fn a_radial_gradient_radiates_from_its_centre() {
    with_exporter(|exporter| {
        let area = Rect::new(150.0, 100.0, 350.0, 300.0);
        let scene = staged(Gradient::radial(RED, BLUE, area), area);
        let frame = render(&scene, exporter);

        let centre = redness(frame.pixel(250, 200));
        assert!(centre > 0.8, "the centre should be red, was {centre:.3}");

        // Every direction cools off the same way. A radial gradient that had
        // been given a linear brush would pass one of these and fail the rest.
        for (x, y, name) in [
            (250, 105, "above"),
            (250, 295, "below"),
            (155, 200, "left"),
            (345, 200, "right"),
        ] {
            let edge = redness(frame.pixel(x, y));
            assert!(
                edge < -0.6,
                "the rim {name} the centre should be blue, was {edge:.3}"
            );
        }
    });
}

/// Stop offsets place the colours. A ramp whose middle stop sits at 0.25 must
/// reach its middle colour a quarter of the way across, not halfway — the
/// distinction the XFL importer used to throw away.
#[test]
fn stop_offsets_decide_where_a_colour_lands() {
    with_exporter(|exporter| {
        let area = Rect::new(50.0, 150.0, 450.0, 250.0);
        let mut gradient = Gradient::new(
            GradientKind::Linear,
            vec![
                GradientStop::new(0.0, RED),
                GradientStop::new(0.25, BLUE),
                GradientStop::new(1.0, BLUE),
            ],
        );
        gradient.fit_to(area);
        let scene = staged(gradient, area);
        let frame = render(&scene, exporter);

        let y = 200;
        // A quarter across is x = 50 + 400/4 = 150. It should already be blue,
        // and stay blue for the whole remaining three quarters.
        assert!(
            redness(frame.pixel(155, y)) < -0.8,
            "the ramp should have reached blue by a quarter across"
        );
        assert!(
            redness(frame.pixel(300, y)) < -0.8,
            "past the last moving stop the colour should hold"
        );
        // Close to the edge, because the ramp is *steep*: it covers the whole
        // of red-to-blue in the first quarter, so by x = 60 it is already a
        // tenth of the way over. That steepness is the point of the test.
        assert!(
            redness(frame.pixel(52, y)) > 0.9,
            "the left edge should still be red, was {:.3}",
            redness(frame.pixel(52, y))
        );
    });
}

/// Repeat and Reflect describe what happens outside the ramp. With the ramp
/// squeezed into the middle of the shape, the two are told apart by what
/// appears at the edges: Reflect mirrors, so it comes back to the start colour;
/// Repeat starts over, so it jumps.
#[test]
fn the_spread_mode_decides_what_happens_past_the_ends() {
    with_exporter(|exporter| {
        let area = Rect::new(50.0, 150.0, 450.0, 250.0);
        // The ramp occupies the middle fifth of the shape, leaving plenty of
        // room either side for the spread to show.
        let ramp = Rect::new(210.0, 150.0, 290.0, 250.0);

        let mut pad = Gradient::linear(RED, BLUE, ramp);
        pad.spread = GradientSpread::Pad;
        let padded = render(&staged(pad, area), exporter);

        let mut reflect = Gradient::linear(RED, BLUE, ramp);
        reflect.spread = GradientSpread::Reflect;
        let reflected = render(&staged(reflect, area), exporter);

        let y = 200;
        // Under Pad the far right is the end colour, blue, all the way out.
        assert!(
            redness(padded.pixel(440, y)) < -0.8,
            "Pad should hold the end colour"
        );
        // Under Reflect the ramp mirrors: one ramp-width past the end is back
        // at red. The ramp is 80 wide, so x = 290 + 40 is mid-mirror and
        // x = 290 + 80 = 370 is fully red again.
        assert!(
            redness(reflected.pixel(368, y)) > 0.8,
            "Reflect should mirror back to the start colour, was {:.3}",
            redness(reflected.pixel(368, y))
        );
    });
}

/// A gradient must not leak into a document that has none. Every other paint in
/// the file still renders exactly as it did, which is what makes this feature
/// safe to add to an existing document.
#[test]
fn a_solid_fill_is_unaffected_by_the_gradient_path() {
    with_exporter(|exporter| {
        let mut scene = Scene::default();
        scene.stage_mut().background = Color::WHITE;
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(100.0, 100.0, 300.0, 300.0).to_path(1e-9), RED),
        );
        let frame = render(&scene, exporter);

        for (x, y) in [(110, 110), (200, 200), (290, 290)] {
            let p = frame.pixel(x, y);
            assert_eq!(
                [p[0], p[1], p[2]],
                [255, 0, 0],
                "a solid fill must stay exactly solid at ({x}, {y})"
            );
        }
    });
}
