//! **A staged scene renders, and a performance moves what is in it.**
//!
//! `buzz-act`'s own tests prove the arithmetic: the feet land on the ground,
//! the knees fold one way, the keys reach the timeline. None of that proves the
//! result is a *picture*. This renders the frames through the exporter and reads
//! the pixels back, which is the only thing that catches a figure built entirely
//! off the stage, a backdrop drawn over the cast, or a walk whose poses are all
//! identical.
//!
//! Skips with no GPU, like every other headless test here.

use buzz_act::{Action, Performance, SceneRecipe, Setting};
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_render::GpuPreference;
use buzz_scene::Scene;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping staged-scene test: no usable GPU ({e})"),
    }
}

fn staged(cast: usize) -> (Scene, Vec<buzz_scene::ObjectId>) {
    let mut scene = Scene::default();
    let built = buzz_act::stage_scene(
        &mut scene,
        &SceneRecipe {
            setting: Setting::Sunset,
            cast,
            frames: 48,
            ..SceneRecipe::default()
        },
    );
    let actors: Vec<_> = built.actors().collect();
    (scene, actors)
}

/// How much of the frame is not the background: a crude "is there anything
/// drawn here" that cannot be fooled by a figure rendered one pixel wide.
fn drawn_fraction(frame: &Frame, against: &Frame) -> f64 {
    let mut different = 0.0;
    let n = (frame.width * frame.height) as f64;
    for (a, b) in frame.pixels.chunks(4).zip(against.pixels.chunks(4)) {
        let delta = (0..3)
            .map(|i| (a[i] as i32 - b[i] as i32).abs())
            .max()
            .unwrap_or(0);
        if delta > 6 {
            different += 1.0;
        }
    }
    different / n
}

/// The people are really in the picture, and they are a substantial part of it
/// rather than a speck in a corner.
#[test]
fn the_cast_is_visible_in_the_rendered_frame() {
    with_exporter(|exporter| {
        let (with_cast, actors) = staged(2);
        assert_eq!(actors.len(), 2);
        let (empty, _) = staged(0);

        let settings = ExportSettings::for_stage(&with_cast);
        let a = exporter.render(&empty, 0, &settings).expect("empty scene");
        let b = exporter
            .render(&with_cast, 0, &settings)
            .expect("scene with a cast");

        let covered = drawn_fraction(&b, &a);
        assert!(
            covered > 0.01,
            "the cast should cover a real part of the frame, got {covered:.4}"
        );
        assert!(
            covered < 0.6,
            "and not swallow it \u{2014} something is wrong with the scale, got {covered:.4}"
        );
    });
}

/// An empty scene is still a scene: a backdrop and a ground, so the frame is
/// not the stage's blank colour.
#[test]
fn even_an_empty_scene_has_a_background() {
    with_exporter(|exporter| {
        let bare = Scene::default();
        let (staged, _) = staged(0);
        let settings = ExportSettings::for_stage(&staged);

        let a = exporter.render(&bare, 0, &settings).expect("bare document");
        let b = exporter.render(&staged, 0, &settings).expect("staged");
        let covered = drawn_fraction(&b, &a);
        assert!(
            covered > 0.5,
            "a sky and a ground should fill the frame, got {covered:.4}"
        );
    });
}

/// **The walk actually walks.** Two frames a few apart must differ, or the
/// performance wrote the same pose onto every key — which every test that only
/// counts keyframes would happily pass.
#[test]
fn a_walk_changes_the_picture_from_frame_to_frame() {
    with_exporter(|exporter| {
        let (mut scene, actors) = staged(1);
        let who = actors[0];
        buzz_act::perform(
            &mut scene,
            who,
            &Performance {
                distance: 260.0,
                ..Performance::new(Action::Walk, 0..24)
            },
        )
        .expect("the walk applies");

        let settings = ExportSettings::for_stage(&scene);
        let first = exporter.render(&scene, 0, &settings).expect("frame 0");
        let later = exporter.render(&scene, 8, &settings).expect("frame 8");

        let moved = drawn_fraction(&later, &first);
        assert!(
            moved > 0.005,
            "the figure should have moved between frames, got {moved:.4}"
        );
    });
}

/// An idle moves too, but far less than a walk: the difference between a held
/// drawing and a dead one is meant to be small, and a test that could not tell
/// them apart would not be testing anything.
#[test]
fn an_idle_moves_less_than_a_walk() {
    with_exporter(|exporter| {
        let measure = |exporter: &mut Exporter, action: Action| {
            let (mut scene, actors) = staged(1);
            buzz_act::perform(
                &mut scene,
                actors[0],
                &Performance {
                    distance: 0.0,
                    ..Performance::new(action, 0..24)
                },
            )
            .expect("applies");
            let settings = ExportSettings::for_stage(&scene);
            let a = exporter.render(&scene, 0, &settings).expect("frame 0");
            let b = exporter.render(&scene, 8, &settings).expect("frame 8");
            drawn_fraction(&b, &a)
        };

        let idle = measure(exporter, Action::Idle);
        let walk = measure(exporter, Action::Walk);
        assert!(
            walk > idle,
            "a walk should disturb more of the frame than a breath: \
             walk {walk:.4}, idle {idle:.4}"
        );
    });
}
