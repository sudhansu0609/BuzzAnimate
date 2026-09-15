//! **Follow-through changes the picture**, on the real GPU.
//!
//! The spring solver and the baker have their own unit tests; what none of them
//! can say is that the lagging poses the baker writes actually come out as
//! *different pixels* when the rig is rendered. A staged character's body is
//! swung, then follow-through is baked onto one arm — the arm must land somewhere
//! visibly different from where the rigid pose had it, or the feature is doing
//! nothing the screen can see.
//!
//! Skips with no GPU, like every other headless test here.

use buzz_act::{Joint, SceneRecipe, Setting, Spring, follow_through_bake};
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_render::GpuPreference;
use buzz_scene::{ObjectId, ObjectKind, Scene, Tween};

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping follow-through test: no usable GPU ({e})"),
    }
}

/// A daylight scene with one rigged actor standing in it, and that actor's id.
fn staged_actor() -> (Scene, ObjectId) {
    let mut scene = Scene::default();
    let actor = {
        let built = buzz_act::stage_scene(
            &mut scene,
            &SceneRecipe {
                setting: Setting::Daylight,
                cast: 1,
                frames: 48,
                ..SceneRecipe::default()
            },
        );
        built.actors().next().expect("one actor was staged")
    };
    (scene, actor)
}

/// Swing the whole body by rotating the hips from its rest by `swing`, reaching
/// it at frame `at` and holding out to `hold_to`.
fn sway_the_body(scene: &mut Scene, actor: ObjectId, at: u32, hold_to: u32, swing: f64) {
    let layer = scene.find_object(actor).expect("the actor").0;
    scene.update_layer(layer, |l| {
        if l.frames.length() <= at {
            l.frames.insert_frame(at);
        }
    });
    scene.ensure_keyframe(layer, at);
    scene.update_object_at(at, actor, |o| {
        if let ObjectKind::Armature(rig) = &mut o.kind {
            let mut pose = rig.armature.pose();
            pose[Joint::Hips.index()] += swing;
            rig.armature.set_pose(&pose);
        }
    });
    scene.update_layer(layer, |l| {
        l.frames.set_tween(0, Tween::motion());
        if l.frames.length() <= hold_to {
            l.frames.insert_frame(hold_to);
        }
    });
}

fn luma(px: &[u8]) -> f64 {
    0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64
}

fn changed_pixels(a: &Frame, b: &Frame) -> usize {
    let mut changed = 0;
    for i in (0..a.pixels.len().min(b.pixels.len())).step_by(4) {
        if (luma(&a.pixels[i..i + 4]) - luma(&b.pixels[i..i + 4])).abs() > 6.0 {
            changed += 1;
        }
    }
    changed
}

/// Baking follow-through onto the arm makes the rendered arm land in a different
/// place at the same frame — the lag is on screen, not just in the numbers.
#[test]
fn baked_follow_through_moves_pixels() {
    with_exporter(|exporter| {
        let (mut scene, actor) = staged_actor();
        sway_the_body(&mut scene, actor, 10, 40, 0.6);
        let settings = ExportSettings::for_stage(&scene);

        // Just after the body settles, the rigid arm has snapped to its pose.
        let before = exporter.render(&scene, 14, &settings).expect("frame 14");

        // Spring the left arm (shoulder + elbow) against that same motion.
        let report = follow_through_bake(
            &mut scene,
            actor,
            Joint::ShoulderL.index(),
            Spring::tail(),
            0..40,
            2,
            0.0,
        )
        .expect("follow-through baked");
        assert!(report.keyframes > 1, "the bake wrote no animation");

        let after = exporter.render(&scene, 14, &settings).expect("frame 14 again");

        let moved = changed_pixels(&before, &after);
        assert!(
            moved > 60,
            "follow-through changed only {moved} pixels — the sprung arm is not \
             visibly different from the rigid one"
        );
    });
}

/// The far end of the range, held, is where the spring has settled: the arm is
/// back on its rigid pose, so almost nothing differs from a scene without the
/// bake there. Guards against a spring that never settles (a jitter that would
/// read as a permanently broken arm).
#[test]
fn a_settled_arm_matches_the_rigid_pose() {
    with_exporter(|exporter| {
        let (mut scene, actor) = staged_actor();
        sway_the_body(&mut scene, actor, 10, 80, 0.6);
        let settings = ExportSettings::for_stage(&scene);

        let rigid_late = exporter.render(&scene, 78, &settings).expect("frame 78");
        follow_through_bake(&mut scene, actor, Joint::ShoulderL.index(), Spring::stiff(), 0..80, 2, 0.0)
            .expect("baked");
        let sprung_late = exporter.render(&scene, 78, &settings).expect("frame 78 again");

        // A near-critical spring, long after the body stopped, sits on the pose.
        let moved = changed_pixels(&rigid_late, &sprung_late);
        assert!(
            moved < 40,
            "the arm never settled: {moved} pixels still differ long after the motion"
        );
    });
}
