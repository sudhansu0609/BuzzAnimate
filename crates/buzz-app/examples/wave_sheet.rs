//! Render the wave brushes, for looking at.
//!
//! `cargo run -p buzz-app --example wave_sheet -- <out-dir>`
//!
//! Not a test: what it produces is a picture, and the only thing that can
//! judge a picture is somebody looking at it. The assertions a machine *can*
//! make about a wave live in `buzz-scene`'s unit tests and in
//! `buzz-export/tests/headless_wave.rs`.
//!
//! Writes one sheet per kind — every kind on its own stage, drawn with the
//! gesture it is meant for — plus three frames of a plume, which is the only
//! way to see that the thing actually moves.

use buzz_export::{ExportSettings, Exporter};
use buzz_geom::{Conditioning, Point, StrokeSample};
use buzz_render::GpuPreference;
use buzz_scene::{ArtPiece, LayerKind, Object, Scene, WaveKind, WaveStroke, wave_artwork};
use peniko::Color;

/// A drag from `a` to `b`, bowed sideways by `bow` so the stroke is a curve
/// rather than a ruler line — which is how anyone actually draws one.
fn drag(a: Point, b: Point, bow: f64) -> Vec<StrokeSample> {
    (0..80)
        .map(|i| {
            let t = i as f64 / 79.0;
            let straight = a.lerp(b, t);
            // A single arch, zero at both ends.
            let push = (t * std::f64::consts::PI).sin() * bow;
            StrokeSample::new(Point::new(straight.x + push, straight.y), t)
        })
        .collect()
}

fn stage(background: Color) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = background;
    scene
}

/// Draw one wave onto a scene at `phase`, as the editor commits one.
fn draw(scene: &mut Scene, kind: WaveKind, samples: &[StrokeSample], size: f64, color: Color, phase: f64) {
    let layer = scene.add_layer(kind.label(), LayerKind::Normal);
    let pieces = wave_artwork(
        kind,
        &WaveStroke {
            samples,
            size,
            color,
            conditioning: Conditioning::smoothing(0.5),
            settings: kind.preset(),
        },
        phase,
    );
    let children: Vec<std::sync::Arc<Object>> = pieces
        .iter()
        .map(|piece| {
            let ArtPiece::Shape(shape) = piece else {
                unreachable!("a wave is vector artwork")
            };
            let id = scene.next_object_id();
            std::sync::Arc::new(Object::shape(id, shape.clone()))
        })
        .collect();
    let id = scene.next_object_id();
    scene.add_object(layer, Object::group(id, children));
}

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out)?;

    let mut exporter = Exporter::new(&GpuPreference::Automatic)?;
    let mut write = |scene: &Scene, name: &str| -> anyhow::Result<()> {
        let settings = ExportSettings::for_stage(scene);
        let path = out.join(format!("{name}.png"));
        exporter.render(scene, 0, &settings)?.write_png(&path)?;
        println!("wrote {}", path.display());
        Ok(())
    };

    // Smoke rises from a point, leaning as it goes.
    let mut scene = stage(Color::from_rgb8(0x10, 0x10, 0x16));
    draw(
        &mut scene,
        WaveKind::Smoke,
        &drag(Point::new(275.0, 390.0), Point::new(275.0, 40.0), 20.0),
        34.0,
        Color::from_rgb8(0xDC, 0xE2, 0xEC),
        0.0,
    );
    write(&scene, "wave-smoke")?;

    // A river runs across the frame.
    let mut scene = stage(Color::from_rgb8(0x14, 0x22, 0x2C));
    draw(
        &mut scene,
        WaveKind::River,
        &drag(Point::new(10.0, 200.0), Point::new(540.0, 210.0), 24.0),
        30.0,
        Color::from_rgb8(0x4E, 0x9A, 0xD6),
        0.0,
    );
    write(&scene, "wave-river")?;

    // Hair falls from a parting.
    let mut scene = stage(Color::from_rgb8(0xE8, 0xE2, 0xD8));
    draw(
        &mut scene,
        WaveKind::Hair,
        &drag(Point::new(240.0, 40.0), Point::new(300.0, 380.0), 40.0),
        40.0,
        Color::from_rgb8(0x5A, 0x33, 0x1C),
        0.0,
    );
    write(&scene, "wave-hair")?;

    // A plain ribbon, for the generic case.
    let mut scene = stage(Color::from_rgb8(0x1A, 0x16, 0x22));
    draw(
        &mut scene,
        WaveKind::Ribbon,
        &drag(Point::new(20.0, 200.0), Point::new(530.0, 200.0), -30.0),
        34.0,
        Color::from_rgb8(0xE0, 0x8C, 0x4A),
        0.0,
    );
    write(&scene, "wave-ribbon")?;

    // And the point of the whole thing: the same plume at three phases.
    for (i, phase) in [0.0, 0.33, 0.66].into_iter().enumerate() {
        let mut scene = stage(Color::from_rgb8(0x10, 0x10, 0x16));
        draw(
            &mut scene,
            WaveKind::Smoke,
            &drag(Point::new(275.0, 390.0), Point::new(275.0, 40.0), 20.0),
            34.0,
            Color::from_rgb8(0xDC, 0xE2, 0xEC),
            phase,
        );
        write(&scene, &format!("wave-smoke-phase-{i}"))?;
    }

    Ok(())
}
