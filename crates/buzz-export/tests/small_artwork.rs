//! **Artwork a few pixels across keeps its colours.** `docs/BARISH1_REGRESSION_PLAN.md`.
//!
//! On barish1's stage the face is drawn at about 0.13 of its size and the
//! eyes at a further 0.34–0.44 inside it: an iris about four pixels across,
//! teeth under three pixels tall, nose lines well under one. Animate keeps
//! all of them — brown iris, grey sclera, white teeth, dark one-pixel lines.
//! We drew black discs and lost the teeth and the lines, for three reasons,
//! each measured here against the same artwork rendered large and averaged
//! down (the coverage the small picture ought to have):
//!
//! 1. **A stroke ignored its placement's scale.** A 2-unit outline inside a
//!    symbol shrunk to an eighth stayed 2 px wide.
//! 2. **The seam seal was a fixed 0.9 px**, which grows a 3.75 px iris's area
//!    by half.
//! 3. **The seal was drawn over the shape before it**, so a colour region
//!    swallowed half a pixel of the line work under it from each side.
//!
//! Plus the guard the fix must not break: a shape at normal size is drawn
//! exactly as before.
//!
//! Skips cleanly when no GPU is available.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Circle, Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{
    FillSpec, LayerKind, Object, ObjectId, Scene, ShapeData, StrokeSpec, SymbolId, SymbolKind,
};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping small-artwork test: no usable GPU ({e})"),
    }
}

fn stage(width: f64, height: f64, background: Color) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = background;
    scene.stage_mut().size = buzz_geom::Size::new(width, height);
    scene
}

fn shape(id: u64, path: buzz_geom::BezPath, fill: Option<Color>, stroke: Option<(Color, f64)>) -> Object {
    Object::shape(
        ObjectId(id),
        ShapeData {
            path,
            fill: fill.map(FillSpec::solid),
            stroke: stroke.map(|(c, w)| StrokeSpec::new(c, w)),
            blend: buzz_scene::PaintBlend::Normal,
        },
    )
}

fn disc(x: f64, y: f64, r: f64) -> buzz_geom::BezPath {
    Circle::new(Point::new(x, y), r).to_path(1e-3)
}

/// barish1's eyeball: a brown iris 30 across, a black pupil, a white glint,
/// and a 2-unit black outline round the iris — carried through `placement`
/// here, geometry *and* pen, so the same eyeball can be drawn straight onto
/// the stage at its final size as the reference.
fn eyeball_objects(placement: Affine) -> Vec<Object> {
    let brown = Color::from_rgb8(0x45, 0x22, 0x15);
    let [a, b, c, d, _, _] = placement.as_coeffs();
    let pen = (a * d - b * c).abs().sqrt();
    vec![
        shape(1, placement * disc(15.0, 15.0, 15.0), Some(brown), None),
        shape(2, placement * disc(16.0, 16.0, 5.8), Some(Color::BLACK), None),
        shape(3, placement * disc(19.0, 12.0, 3.1), Some(Color::WHITE), None),
        shape(4, placement * disc(15.0, 15.0, 15.0), None, Some((Color::BLACK, 2.0 * pen))),
    ]
}

/// The eyeball as a symbol, in its own units.
fn eyeball_symbol(scene: &mut Scene) -> SymbolId {
    let objects = eyeball_objects(Affine::IDENTITY);
    let id = scene.add_symbol("eyeball", SymbolKind::Graphic, None);
    let layer = scene.library().get(id).unwrap().layers.iter().next().unwrap().id;
    scene.library_mut().update(id, |s| {
        s.layers.update(layer, |l| {
            l.frames.set_objects(0, objects.into_iter().map(Arc::new).collect());
        });
    });
    id
}

fn render(scene: &Scene, exporter: &mut Exporter, factor: f64) -> Frame {
    exporter
        .render(scene, 0, &ExportSettings::scaled(scene, factor))
        .expect("render")
}

fn luma(p: [u8; 4]) -> f64 {
    0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64
}

/// Mean luma of `small` over the pixel box, and of `big` over the same part
/// of the stage, averaged down by `ratio` (big pixels per small pixel).
fn mean_luma_pair(small: &Frame, big: &Frame, ratio: u32, (x0, y0, x1, y1): (u32, u32, u32, u32)) -> (f64, f64) {
    let (mut s, mut b, mut n, mut nb) = (0.0, 0.0, 0.0, 0.0);
    for y in y0..y1 {
        for x in x0..x1 {
            s += luma(small.pixel(x, y));
            n += 1.0;
            for yy in y * ratio..(y + 1) * ratio {
                for xx in x * ratio..(x + 1) * ratio {
                    b += luma(big.pixel(xx, yy));
                    nb += 1.0;
                }
            }
        }
    }
    (s / n, b / nb)
}

/// **The eyeball at stage size.** Placed at a quarter inside a stage rendered
/// at a half — an iris 3.75 px across, its outline a quarter of a pixel —
/// the small picture must have the coverage of the large one averaged down:
/// within 10 % of its mean luma. Before the fix it came out a black disc,
/// 40 % too dark. Checked with the symbol cache both on and off, since a
/// stamped symbol bakes its stroke widths at a quantised scale.
#[test]
fn a_small_eyeball_keeps_its_iris() {
    with_exporter(|exporter| {
        // Off the pixel grid on purpose.
        let placement = Affine::translate((96.3, 44.7)) * Affine::scale(0.25);
        let mut scene = stage(200.0, 100.0, Color::WHITE);
        let eye = eyeball_symbol(&mut scene);
        let layer = scene.add_layer("Eyes", LayerKind::Normal);
        scene.add_instance_at(layer, 0, eye, placement);

        // The reference: the same eyeball drawn straight on the stage at its
        // final size, rendered 32 times larger and averaged down.
        let mut reference = stage(200.0, 100.0, Color::WHITE);
        let layer = reference.add_layer("Eyes", LayerKind::Normal);
        for object in eyeball_objects(placement) {
            reference.add_object(layer, object);
        }
        let big = render(&reference, exporter, 16.0);

        for reuse in [true, false] {
            exporter.set_symbol_reuse(reuse);
            let small = render(&scene, exporter, 0.5);
            // The eyeball spans stage x 96.3..103.8, y 44.7..52.2: small
            // pixels 48..52 by 22..27, with a pixel of margin.
            let (s, b) = mean_luma_pair(&small, &big, 32, (47, 21, 53, 28));
            eprintln!("eyeball (reuse {reuse}): small {s:.1}, reference {b:.1}");
            assert!(
                (s - b).abs() <= 0.10 * b,
                "a 3.75 px eyeball must keep its coverage (reuse {reuse}): mean luma {s:.1} vs {b:.1} \
                 averaged down from the large render"
            );
        }
        exporter.set_symbol_reuse(true);
    });
}

/// **A stroke scales with its symbol.** A 2-unit outline inside a symbol
/// placed at an eighth is a quarter of a pixel wide on a stage rendered at
/// 1:1 — so the ring of ink it leaves is an eighth of what the unscaled
/// 2 px pen used to leave.
#[test]
fn a_stroke_scales_with_its_symbol() {
    with_exporter(|exporter| {
        let mut scene = stage(200.0, 100.0, Color::WHITE);
        let ring = scene.add_symbol("ring", SymbolKind::Graphic, None);
        let layer = scene.library().get(ring).unwrap().layers.iter().next().unwrap().id;
        let ring_object = shape(1, disc(0.0, 0.0, 200.0), None, Some((Color::BLACK, 2.0)));
        scene.library_mut().update(ring, |s| {
            s.layers.update(layer, |l| {
                l.frames.set_objects(0, vec![Arc::new(ring_object)]);
            });
        });
        let stage_layer = scene.add_layer("Ring", LayerKind::Normal);
        scene.add_instance_at(stage_layer, 0, ring, Affine::translate((100.0, 50.0)) * Affine::scale(0.125));

        for reuse in [true, false] {
            exporter.set_symbol_reuse(reuse);
            let frame = render(&scene, exporter, 1.0);
            // Ink, in pixels: the sum of darkness over the whole frame.
            let ink: f64 = (0..frame.height)
                .flat_map(|y| (0..frame.width).map(move |x| (x, y)))
                .map(|(x, y)| 1.0 - luma(frame.pixel(x, y)) / 255.0)
                .sum();
            // A ring of radius 25 px, 0.25 px wide.
            let expected = 2.0 * std::f64::consts::PI * 25.0 * 0.25;
            eprintln!("ring ink (reuse {reuse}): {ink:.1} px, expected {expected:.1}");
            assert!(
                (ink - expected).abs() <= 0.25 * expected,
                "a 2-unit stroke in a symbol at 1/8 must be a quarter pixel wide (reuse {reuse}): \
                 {ink:.1} px of ink, expected {expected:.1}"
            );
        }
        exporter.set_symbol_reuse(true);
    });
}

/// **Line work under colour regions survives.** A dark strip 2 units wide,
/// drawn first, with two light regions drawn after it abutting it on either
/// side — the order imported artwork draws a face in. At a quarter scale the
/// strip is half a pixel wide and must still be there: the columns across it
/// within 10 % of the large render averaged down. Before the fix each
/// region's seal covered the strip from both sides and it vanished.
#[test]
fn a_thin_line_between_two_regions_survives() {
    with_exporter(|exporter| {
        let mut scene = stage(200.0, 100.0, Color::WHITE);
        let layer = scene.add_layer("Face", LayerKind::Normal);
        let skin = Color::from_rgb8(0xE5, 0x95, 0x59);
        let (x0, x1) = (98.6, 100.6);
        scene.add_object(layer, shape(1, Rect::new(x0, 20.0, x1, 80.0).to_path(1e-9), Some(Color::BLACK), None));
        scene.add_object(layer, shape(2, Rect::new(60.0, 20.0, x0, 80.0).to_path(1e-9), Some(skin), None));
        scene.add_object(layer, shape(3, Rect::new(x1, 20.0, 140.0, 80.0).to_path(1e-9), Some(skin), None));

        let big = render(&scene, exporter, 8.0);
        let small = render(&scene, exporter, 0.25);
        // Stage x 96..104 is small columns 24..26; rows well inside.
        let (s, b) = mean_luma_pair(&small, &big, 32, (24, 8, 26, 17));
        eprintln!("thin line: small {s:.1}, reference {b:.1}");
        assert!(
            (s - b).abs() <= 0.10 * b,
            "a half-pixel line between two regions must keep its coverage: mean luma {s:.1} vs {b:.1}"
        );
        // And the seam between the regions and the line stays closed: no
        // white (the stage) anywhere inside the artwork.
        for y in 8..17 {
            for x in 17..33 {
                let p = small.pixel(x, y);
                assert!(
                    p[0].min(p[1]).min(p[2]) < 0xF0,
                    "the stage shows through at ({x}, {y}): {p:?}"
                );
            }
        }
    });
}

/// **Normal-size artwork is drawn exactly as before.** A 200-px square, off
/// the pixel grid: its edge pixels are the ones the full seal gave before
/// this change (measured on the pre-fix build; ±1 for driver rounding). A
/// shape this size takes the whole seal, and a seal beneath a lone shape
/// composites to the same pixels as one drawn over it.
#[test]
fn a_200px_shape_is_drawn_as_before() {
    with_exporter(|exporter| {
        let mut scene = stage(400.0, 300.0, Color::WHITE);
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let red = Color::from_rgb8(0xC0, 0x20, 0x20);
        scene.add_object(layer, shape(1, Rect::new(50.3, 50.3, 250.6, 250.6).to_path(1e-9), Some(red), None));
        let frame = render(&scene, exporter, 1.0);
        let probes = [(49, 150), (50, 150), (51, 150), (249, 150), (250, 150), (251, 150), (150, 49), (150, 50), (150, 250), (150, 251)];
        let got: Vec<[u8; 4]> = probes.iter().map(|&(x, y)| frame.pixel(x, y)).collect();
        eprintln!("edge pixels: {got:?}");
        for (i, (g, w)) in got.iter().zip(GOLDEN.iter()).enumerate() {
            for c in 0..4 {
                assert!(
                    (g[c] as i32 - w[c] as i32).abs() <= 1,
                    "edge pixel {:?} changed: {g:?}, was {w:?}",
                    probes[i]
                );
            }
        }
    });
}

/// Measured on the pre-fix build (`SEAM_SEAL_PX` drawn in line, no cap), at
/// the probes above.
const GOLDEN: [[u8; 4]; 10] = [
    [248, 229, 229, 255],
    [196, 47, 47, 255],
    [192, 32, 32, 255],
    [192, 32, 32, 255],
    [196, 46, 46, 255],
    [252, 244, 244, 255],
    [246, 222, 222, 255],
    [197, 49, 49, 255],
    [196, 45, 45, 255],
    [252, 244, 244, 255],
];
