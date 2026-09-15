//! **A wave renders, and it moves**, on the real GPU.
//!
//! [`buzz_scene::wave`] has unit tests for every property of its geometry;
//! what none of them can say is that the *renderer* draws the result. A plume
//! is a gradient-filled band inside a group on a normal layer, and a gradient
//! whose transform was built by hand is exactly the kind of fill that can come
//! out empty — or, worse, come out as one flat colour, which looks fine in a
//! bounding box and wrong on the screen.
//!
//! And a wave is an animation, so there is a second thing to check that a
//! still cannot: consecutive frames must actually *differ* on screen, and the
//! frame after the last must be the first again. A cycle that jolts once every
//! time round is the one defect a looping plume cannot have, and pixels are
//! the only place it can be seen.
//!
//! Skips with no GPU, like every other headless test here.

use std::sync::Arc;

use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::Point;
use buzz_render::GpuPreference;
use buzz_scene::{ArtPiece, LayerKind, Object, Scene, WaveKind, WaveStroke, wave_loop};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping wave test: no usable GPU ({e})"),
    }
}

/// An upward drag through the middle of the stage — a plume rising.
fn rising() -> Vec<buzz_geom::StrokeSample> {
    (0..60)
        .map(|i| {
            let t = i as f64 / 59.0;
            buzz_geom::StrokeSample::new(Point::new(275.0, 380.0 - t * 300.0), t)
        })
        .collect()
}

/// A dark stage with one wave baked across its frames, committed the way the
/// editor commits one: a keyframe per frame, each holding a group.
fn document(kind: WaveKind, color: Color, frames: u32) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
    let layer = scene.add_layer("Wave", LayerKind::Normal);

    let samples = rising();
    let mut settings = kind.preset();
    settings.frames = frames;
    let cycle = wave_loop(
        kind,
        &WaveStroke {
            samples: &samples,
            size: 30.0,
            color,
            conditioning: buzz_geom::Conditioning::smoothing(0.5),
            settings,
        },
    );
    assert_eq!(cycle.len(), frames as usize);

    for (i, pieces) in cycle.iter().enumerate() {
        assert!(!pieces.is_empty(), "{kind:?} frame {i} made nothing to draw");
        let frame = i as u32;
        scene.update_layer(layer, |l| {
            l.frames.insert_blank_keyframe(frame);
        });

        let children: Vec<Arc<Object>> = pieces
            .iter()
            .map(|piece| {
                let ArtPiece::Shape(shape) = piece else {
                    panic!("a wave should be vector artwork");
                };
                let id = scene.next_object_id();
                Arc::new(Object::shape(id, shape.clone()))
            })
            .collect();
        let id = scene.next_object_id();
        scene.add_object_at(layer, frame, Object::group(id, children));
    }
    scene
}

fn luma(px: &[u8]) -> f64 {
    0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64
}

/// Pixels brighter than the stage, in the band the wave was drawn through.
fn lit_pixels(frame: &Frame) -> usize {
    let mut lit = 0;
    for y in 60..400u32 {
        for x in 150..410u32 {
            let i = ((y * frame.width + x) * 4) as usize;
            if i + 3 < frame.pixels.len() && luma(&frame.pixels[i..i + 4]) > 40.0 {
                lit += 1;
            }
        }
    }
    lit
}

/// How much two frames differ, as a count of pixels that changed noticeably.
fn changed_pixels(a: &Frame, b: &Frame) -> usize {
    let mut changed = 0;
    for i in (0..a.pixels.len().min(b.pixels.len())).step_by(4) {
        if (luma(&a.pixels[i..i + 4]) - luma(&b.pixels[i..i + 4])).abs() > 6.0 {
            changed += 1;
        }
    }
    changed
}

/// Every kind must actually land on the stage. A wave whose bounding box is
/// right and whose pixels are absent is the failure this catches.
#[test]
fn every_wave_draws_something_on_the_stage() {
    with_exporter(|exporter| {
        for kind in WaveKind::ALL {
            let scene = document(kind, Color::from_rgb8(0xD8, 0xE2, 0xF0), 2);
            let settings = ExportSettings::for_stage(&scene);
            let frame = exporter.render(&scene, 0, &settings).expect("a frame");
            let lit = lit_pixels(&frame);
            assert!(
                lit > 500,
                "{kind:?} drew {lit} visible pixels — the wave did not reach the stage"
            );
        }
    });
}

/// Smoke is the worst case for the renderer: a hand-transformed linear
/// gradient inside a group. It must be a *fade* — bright at the source, gone
/// at the top — rather than one flat tone, which is what a gradient whose
/// transform went wrong looks like.
#[test]
fn a_plume_of_smoke_fades_out_as_it_rises() {
    with_exporter(|exporter| {
        let scene = document(WaveKind::Smoke, Color::from_rgb8(0xE8, 0xE8, 0xF2), 2);
        let settings = ExportSettings::for_stage(&scene);
        let frame = exporter.render(&scene, 0, &settings).expect("a frame");

        // Mean brightness in two bands of the rise: near the source and near
        // the top of the stroke.
        let band = |low: u32, high: u32| -> f64 {
            let (mut sum, mut n) = (0.0, 0.0f64);
            for y in low..high {
                for x in 150..410u32 {
                    let i = ((y * frame.width + x) * 4) as usize;
                    if i + 3 < frame.pixels.len() {
                        sum += luma(&frame.pixels[i..i + 4]);
                        n += 1.0;
                    }
                }
            }
            sum / n.max(1.0)
        };

        let source = band(320, 380);
        let top = band(80, 140);
        assert!(
            source > top + 3.0,
            "smoke should thin out as it rises: {source:.2} at the source, \
             {top:.2} at the top — a flat plume means the ramp did not land"
        );
    });
}

/// The point of a wave: consecutive frames are different pictures, and the
/// difference keeps growing as the flow runs rather than jiggling in place. A
/// cycle that renders the same image every frame is a still with a frame
/// count.
///
/// Measured against the River preset's own cycle: eight frames carrying two
/// whole cycles, so frame 2 is the far side of the first one.
#[test]
fn consecutive_frames_of_a_wave_are_different_pictures() {
    with_exporter(|exporter| {
        let scene = document(WaveKind::River, Color::from_rgb8(0x6C, 0xB8, 0xE8), 8);
        let settings = ExportSettings::for_stage(&scene);

        let first = exporter.render(&scene, 0, &settings).expect("frame 0");
        let next = exporter.render(&scene, 1, &settings).expect("frame 1");
        let opposite = exporter.render(&scene, 2, &settings).expect("frame 2");

        assert!(
            changed_pixels(&first, &next) > 200,
            "frame 1 looks like frame 0 — the current is not running"
        );
        assert!(
            changed_pixels(&first, &opposite) > changed_pixels(&first, &next),
            "the flow should keep moving rather than jiggling in place"
        );
    });
}

/// **The loop closes, in pixels.** River's preset runs two whole cycles across
/// its frames, so frame 4 of eight is exactly one cycle on from frame 0 and
/// has to be the *same picture* — not a similar one. This is the seamless-loop
/// property the generator promises, checked where it can actually be seen.
#[test]
fn a_whole_cycle_later_is_the_very_same_picture() {
    with_exporter(|exporter| {
        let scene = document(WaveKind::River, Color::from_rgb8(0x6C, 0xB8, 0xE8), 8);
        let settings = ExportSettings::for_stage(&scene);
        assert_eq!(
            WaveKind::River.preset().cycles,
            2,
            "this test reads the preset's cycle count"
        );

        let first = exporter.render(&scene, 0, &settings).expect("frame 0");
        let round = exporter.render(&scene, 4, &settings).expect("frame 4");
        assert_eq!(
            changed_pixels(&first, &round),
            0,
            "a whole cycle later the water should be exactly where it started"
        );
    });
}

/// **The loop closes on screen.** The wave is baked over frames `0..n`, so
/// frame `n` — the frame the animation would run into if it were held — has
/// to be the same picture as frame 0, or the plume jolts once per cycle.
///
/// Rendered from a scene whose last frame is followed by the *repeat* of the
/// first, which is what a looping export does.
#[test]
fn a_baked_cycle_returns_to_its_first_frame() {
    with_exporter(|exporter| {
        let frames = 8u32;
        let scene = document(WaveKind::Smoke, Color::from_rgb8(0xE0, 0xE6, 0xF4), frames);
        let settings = ExportSettings::for_stage(&scene);

        let first = exporter.render(&scene, 0, &settings).expect("frame 0");
        let last = exporter
            .render(&scene, frames - 1, &settings)
            .expect("the last frame");

        // The last frame is one step short of a whole turn, so it must differ
        // from the first — and by about as much as any other step does. That
        // is what "seamless" means: no frame is a bigger jump than its
        // neighbours, least of all the wrap.
        let step = changed_pixels(&first, &last);
        let ordinary = changed_pixels(
            &first,
            &exporter.render(&scene, 1, &settings).expect("frame 1"),
        );
        assert!(step > 0, "the cycle never moved at all");
        assert!(
            step < ordinary * 6,
            "the wrap is a far bigger jump ({step}) than an ordinary step \
             ({ordinary}) — the loop does not close"
        );
    });
}
