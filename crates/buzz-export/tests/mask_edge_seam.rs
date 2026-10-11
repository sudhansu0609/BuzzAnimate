//! Step 2 of `docs/RENDER_SEAMS_PLAN.md`, measured: a mask no longer opens a
//! hairline where the masked artwork meets the mask edge.
//!
//! # The defect
//!
//! A mask clips through `SceneBuilder::push_clip`, whose edge is
//! antialiased, and the masked artwork often has its own edge on the very
//! same curve — an eye white cut to the eye shape, or here, a red circle
//! exactly the size of its black mask circle. The two coverages multiply
//! (0.5 × 0.5), so a pixel that should read about half covered reads a
//! quarter covered instead, and the white stage shows through: a pale ring
//! round the masked shape.
//!
//! Skips cleanly when no GPU is available.

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Circle, Point, Shape as _, Size};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

const BLACK: Color = Color::BLACK;
const RED: Color = Color::from_rgb8(0xC0, 0x20, 0x20);

const CX: f64 = 150.0;
const CY: f64 = 150.0;
const RADIUS: f64 = 80.0;

fn circle_path(cx: f64, cy: f64, r: f64) -> buzz_geom::BezPath {
    Circle::new(Point::new(cx, cy), r).to_path(1e-4)
}

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    static SHARED: std::sync::OnceLock<Option<std::sync::Mutex<Exporter>>> =
        std::sync::OnceLock::new();

    let shared = SHARED.get_or_init(|| match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => Some(std::sync::Mutex::new(e)),
        Err(e) => {
            eprintln!("skipping mask seam test: no usable GPU ({e})");
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

/// A red circle, masked by a black circle **exactly the same size and
/// position** — the ring under test. The mask layer must sit above the
/// masked layer in the stack (`Layer::mask_groups`'s rule: a mask claims the
/// unbroken run of `Masked` layers directly beneath it), which is why the
/// masked layer is added first: `Scene::add_layer` always inserts at the top.
fn masked_scene() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(300.0, 300.0);

    let art = scene.add_layer("Art", LayerKind::Masked);
    scene.add_shape(art, ShapeData::filled(circle_path(CX, CY, RADIUS), RED));
    let mask = scene.add_layer("Mask", LayerKind::Mask);
    scene.add_shape(mask, ShapeData::filled(circle_path(CX, CY, RADIUS), BLACK));
    scene
}

/// The same red circle, on the same stage, with no mask at all — the
/// baseline an ordinary single antialiased edge is measured against.
fn unmasked_scene() -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(300.0, 300.0);

    let art = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(art, ShapeData::filled(circle_path(CX, CY, RADIUS), RED));
    scene
}

/// Average of the three colour channels — a scalar "how light is this
/// pixel", which is what the doubled clip coverage raises.
fn lightness(pixel: [u8; 4]) -> f64 {
    (f64::from(pixel[0]) + f64::from(pixel[1]) + f64::from(pixel[2])) / 3.0
}

/// Points sampled round the ring: several angles, and a few pixels either
/// side of the true radius at each, so the sweep crosses the antialiased
/// transition wherever it actually falls.
fn ring_points() -> Vec<(u32, u32)> {
    let mut points = Vec::new();
    for i in 0..32 {
        let theta = std::f64::consts::TAU * f64::from(i) / 32.0;
        for dr in -3..=3 {
            let r = RADIUS + f64::from(dr);
            let x = CX + r * theta.cos();
            let y = CY + r * theta.sin();
            if x >= 0.0 && y >= 0.0 {
                points.push((x.round() as u32, y.round() as u32));
            }
        }
    }
    points
}

fn render(scene: &Scene, exporter: &mut Exporter) -> Frame {
    let settings = ExportSettings::for_stage(scene);
    exporter.render(scene, 0, &settings).expect("render")
}

#[test]
fn a_mask_the_same_size_as_its_artwork_shows_no_light_ring() {
    with_exporter(|exporter| {
        let masked = render(&masked_scene(), exporter);
        let unmasked = render(&unmasked_scene(), exporter);

        // 50% coverage of the red fill over the white stage — what an
        // ordinary, singly antialiased edge looks like at the pixel that
        // straddles the boundary exactly. The old doubled-coverage defect
        // read about a quarter covered there instead, which is lighter.
        let red_bytes = RED.to_rgba8().to_u8_array();
        let red_lightness = lightness([red_bytes[0], red_bytes[1], red_bytes[2], 255]);
        let reference = (255.0 + red_lightness) / 2.0;

        let points = ring_points();

        // Anchor the "no pixel lighter than 50% coverage" claim at the ring
        // pixel closest to that 50% point in the *unmasked* baseline — the
        // one location guaranteed to be near the coverage the defect
        // squares — rather than at an arbitrary band, which would also
        // legitimately include near-white pixels just outside the circle.
        let anchor = points
            .iter()
            .copied()
            .min_by(|a, b| {
                let da = (lightness(unmasked.pixel(a.0, a.1)) - reference).abs();
                let db = (lightness(unmasked.pixel(b.0, b.1)) - reference).abs();
                da.total_cmp(&db)
            })
            .expect("the ring sweep is not empty");
        let anchor_lightness = lightness(masked.pixel(anchor.0, anchor.1));
        assert!(
            anchor_lightness <= reference + 8.0,
            "the masked ring is lighter than 50% coverage at {anchor:?}: \
             {anchor_lightness:.1} against a reference of {reference:.1}"
        );

        // Every ring pixel must match the unmasked baseline closely: the
        // mask changes nothing about how this edge looks once it is sealed.
        let mut worst = 0.0f64;
        let mut worst_at = (0, 0);
        for (x, y) in points {
            let m = masked.pixel(x, y);
            let u = unmasked.pixel(x, y);
            for c in 0..3 {
                let d = (f64::from(m[c]) - f64::from(u[c])).abs();
                if d > worst {
                    worst = d;
                    worst_at = (x, y);
                }
            }
        }
        assert!(
            worst <= 16.0,
            "masked and unmasked disagree by {worst:.1}/255 at {worst_at:?}, \
             more than the 16/255 an unrelated antialiasing wobble should cost"
        );
    });
}
