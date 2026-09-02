//! **Live modifiers change the picture, and survive a save**, on the real GPU.
//!
//! The modifier evaluator has unit tests in `buzz-scene`; what they cannot show
//! is that the *renderer* honours a modifier — one edit in `draw_layer` is meant
//! to reach preview, export and these tests alike — and that a modifier written
//! to a `.buzz` file comes back drawing the same thing. A wiggle must displace a
//! rendered object; a spring must lag a rendered arm; and a saved-then-reloaded
//! scene must render pixel-for-pixel what it did before.
//!
//! Skips with no GPU, like every other headless test here.

use buzz_act::{Joint, SceneRecipe, Setting};
use buzz_export::{ExportSettings, Exporter, Frame};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{LayerKind, Modifier, Object, ObjectId, ObjectKind, Scene, ShapeData, Tween};
use peniko::Color;

fn with_exporter(test: impl FnOnce(&mut Exporter)) {
    match Exporter::new(&GpuPreference::Automatic) {
        Ok(mut e) => test(&mut e),
        Err(e) => eprintln!("skipping modifier test: no usable GPU ({e})"),
    }
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

/// A dark stage with one bright square held across a range, and its id.
fn sign_scene() -> (Scene, ObjectId) {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x0C, 0x0C, 0x12);
    let layer = scene.add_layer("Sign", LayerKind::Normal);
    let mut object = Object::shape(
        ObjectId(1),
        ShapeData::filled(
            Rect::new(-20.0, -20.0, 20.0, 20.0).to_path(1e-9),
            Color::from_rgb8(0xF0, 0xF0, 0xF6),
        ),
    );
    object.transform = Affine::translate((275.0, 200.0));
    let id = scene.add_object(layer, object).expect("on a layer");
    scene.update_layer(layer, |l| {
        l.frames.insert_frame(30);
    });
    (scene, id)
}

#[test]
fn a_live_wiggle_moves_the_rendered_object() {
    with_exporter(|exporter| {
        let (mut scene, id) = sign_scene();
        let settings = ExportSettings::for_stage(&scene);
        let plain = exporter.render(&scene, 10, &settings).expect("frame 10");

        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Wiggle {
                amplitude: 40.0,
                frequency: 2.0,
            });
        });
        let wiggled = exporter.render(&scene, 10, &settings).expect("frame 10 again");

        assert!(
            changed_pixels(&plain, &wiggled) > 200,
            "the live wiggle did not move the object on screen"
        );
    });
}

#[test]
fn a_saved_scene_with_a_modifier_reloads_identically() {
    with_exporter(|exporter| {
        let (mut scene, id) = sign_scene();
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Wiggle {
                amplitude: 40.0,
                frequency: 2.0,
            });
        });
        let settings = ExportSettings::for_stage(&scene);
        let before = exporter.render(&scene, 10, &settings).expect("before save");

        // Through the real .buzz container, not just the DTO.
        let bytes = buzz_doc::format::to_bytes(&scene).expect("save");
        let loaded = buzz_doc::format::from_bytes(&bytes).expect("load");
        let after = exporter.render(&loaded, 10, &settings).expect("after load");

        assert_eq!(
            changed_pixels(&before, &after),
            0,
            "the reloaded scene rendered differently — a modifier did not survive the file"
        );
    });
}

/// A staged actor whose body is swung, plus its id — reused from the baked
/// follow-through test's setup.
fn swung_actor() -> (Scene, ObjectId) {
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
        built.actors().next().expect("an actor")
    };
    let layer = scene.find_object(actor).expect("the actor").0;
    scene.update_layer(layer, |l| {
        if l.frames.length() <= 10 {
            l.frames.insert_frame(10);
        }
    });
    scene.ensure_keyframe(layer, 10);
    scene.update_object_at(10, actor, |o| {
        if let ObjectKind::Armature(rig) = &mut o.kind {
            let mut pose = rig.armature.pose();
            pose[Joint::Hips.index()] += 0.6;
            rig.armature.set_pose(&pose);
        }
    });
    scene.update_layer(layer, |l| {
        l.frames.set_tween(0, Tween::motion());
        if l.frames.length() <= 40 {
            l.frames.insert_frame(40);
        }
    });
    (scene, actor)
}

#[test]
fn a_live_spring_lags_the_rendered_arm() {
    with_exporter(|exporter| {
        let (mut scene, actor) = swung_actor();
        let settings = ExportSettings::for_stage(&scene);
        let rigid = exporter.render(&scene, 14, &settings).expect("rigid frame");

        scene.update_object_across(0, u32::MAX, actor, |o| {
            o.modifiers.push(Modifier::Spring {
                root: Joint::ShoulderL.index(),
                stiffness: 80.0,
                damping: 6.0,
                coupling: 0.0,
            });
        });
        let sprung = exporter.render(&scene, 14, &settings).expect("sprung frame");

        assert!(
            changed_pixels(&rigid, &sprung) > 60,
            "the live spring did not change the arm on screen"
        );
    });
}
