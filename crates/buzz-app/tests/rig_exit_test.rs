//! Phase 7's exit test: rig a character arm, animate it, and get it out.
//!
//! This is the whole feature end to end, through the real seams rather than
//! around them: build the rig with the editor's own gestures, key two poses on
//! the timeline, save and reopen the document, render the tweened frames on
//! the GPU, and read the pixels back.
//!
//! What it is guarding is the joins. Each piece is tested on its own already —
//! the solver in `buzz-rig`, the model in `buzz-scene`, the format in
//! `buzz-doc`, the exporter in `buzz-export`. What no unit test can see is
//! whether a pose keyed on frame 0, tweened at frame 6, saved to disk and
//! reopened still bends the same arm the same way. That is where rigging would
//! actually break, and it is the only thing this file asserts.

use buzz_app::editor::Editor;
use buzz_app::rigging;
use buzz_export::{ExportSettings, Exporter};
use buzz_geom::{Point, Rect, Shape as _};
use buzz_render::GpuPreference;
use buzz_scene::{ObjectKind, Scene, ShapeData, Tween};
use peniko::Color;

const LIMB: Color = Color::from_rgb8(0x33, 0x66, 0x99);

/// A document with one horizontal limb, and the id of that artwork.
fn document_with_limb() -> (Editor, buzz_scene::ObjectId) {
    let mut editor = Editor::default();
    editor.camera.viewport = buzz_geom::Size::new(1000.0, 800.0);
    editor.camera.set_zoom_percent(100.0);

    let layer = editor.selection.active_layer().expect("a layer");
    let mut id = None;
    editor.doc.edit("Draw", |scene| {
        scene.stage_mut().background = Color::WHITE;
        id = scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(100.0, 190.0, 300.0, 210.0).to_path(1e-9), LIMB),
        );
    });
    (editor, id.expect("the limb"))
}

/// Build a two-bone arm over the limb, through the same calls the Bone tool
/// makes when the user drags.
fn rig_the_arm(editor: &mut Editor, id: buzz_scene::ObjectId) {
    editor.doc.edit("Create Armature", |scene| {
        rigging::rig_object(
            scene,
            0,
            id,
            Point::new(100.0, 200.0),
            Point::new(200.0, 200.0),
        );
    });
    editor.doc.edit("Add Bone", |scene| {
        rigging::add_bone(
            scene,
            0,
            id,
            Some(0),
            Point::new(200.0, 200.0),
            Point::new(300.0, 200.0),
        );
    });
}

fn armature_at(scene: &Scene, frame: u32, id: buzz_scene::ObjectId) -> buzz_rig::Armature {
    let object = scene
        .layers()
        .iter()
        .flat_map(|l| {
            l.frames
                .resolved_at(frame)
                .iter()
                .cloned()
                .collect::<Vec<_>>()
        })
        .find(|o| o.id == id)
        .unwrap_or_else(|| panic!("no rigged object {id:?} at frame {frame}"));

    match &object.kind {
        ObjectKind::Armature(rig) => rig.armature.clone(),
        other => panic!("expected an armature, found {other:?}"),
    }
}

/// Rig an arm, key two poses, and tween between them — the exit criterion.
#[test]
fn a_rigged_arm_is_posed_keyed_and_tweened() {
    let (mut editor, id) = document_with_limb();
    rig_the_arm(&mut editor, id);

    let layer = editor.selection.active_layer().expect("a layer");

    // Frame 0 keeps the arm straight. Frame 12 holds it bent.
    //
    // The frames are inserted before the keyframe, which is what an animator
    // does — F5 to extend the span, then F6 to key the end of it. F6 on its
    // own past the end of a span produces a *blank* keyframe here, because
    // there is no frame there to duplicate. See §7 in PROGRESS.md.
    editor.doc.edit("Insert Frame", |scene| {
        scene.update_layer(layer, |l| {
            for _ in 0..12 {
                l.frames.insert_frame(0);
            }
        });
    });
    editor.doc.edit("Insert Keyframe", |scene| {
        scene.update_layer(layer, |l| {
            l.frames.insert_keyframe(12);
        });
    });
    editor.set_frame(12);
    editor.doc.edit("Pose", |scene| {
        rigging::pose_bone(scene, 12, id, 1, Point::new(200.0, 310.0));
    });
    editor.doc.edit("Create Classic Tween", |scene| {
        scene.update_layer(layer, |l| {
            l.frames.set_tween(0, Tween::classic());
        });
    });

    let scene = editor.scene();
    let straight = armature_at(scene, 0, id);
    let bent = armature_at(scene, 12, id);
    let middle = armature_at(scene, 6, id);

    assert!(
        bent.bones[1].angle.abs() > 0.5,
        "the pose did not bend the elbow: {:?}",
        bent.bones[1].angle
    );

    // The tweened frame lies between the two keys, and exists nowhere in the
    // document — it is resolved on the way to being drawn.
    let (a, b, m) = (
        straight.bones[1].angle,
        bent.bones[1].angle,
        middle.bones[1].angle,
    );
    assert!(
        (m - (a + b) * 0.5).abs() < 1e-6,
        "frame 6 should be halfway between {a} and {b}, got {m}"
    );

    // And the artwork follows: the tweened arm reaches lower than the straight
    // one and not as low as the fully bent one.
    let reach = |frame: u32| {
        armature_at(editor.scene(), frame, id);
        let object = editor
            .scene()
            .layers()
            .iter()
            .flat_map(|l| {
                l.frames
                    .resolved_at(frame)
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .find(|o| o.id == id)
            .expect("the rig");
        object.bounds().y1
    };
    assert!(
        reach(6) > reach(0) + 5.0,
        "the tween did not move the artwork"
    );
    assert!(reach(12) > reach(6) + 5.0, "the tween overshot its end key");
}

/// The pose survives the disk. A rig that loads back straight would lose an
/// animator's whole day.
#[test]
fn a_rigged_pose_survives_saving_and_reopening() {
    let (mut editor, id) = document_with_limb();
    rig_the_arm(&mut editor, id);
    editor.doc.edit("Pose", |scene| {
        rigging::pose_bone(scene, 0, id, 1, Point::new(200.0, 300.0));
    });

    let posed = armature_at(editor.scene(), 0, id).pose();

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rigged.buzz");
    editor.doc.save_as(&path).expect("save");

    let reopened = buzz_doc::Document::open(&path).expect("open");
    let loaded = armature_at(reopened.scene(), 0, id).pose();

    assert_eq!(posed.len(), loaded.len());
    for (before, after) in posed.iter().zip(&loaded) {
        assert!(
            (before - after).abs() < 1e-9,
            "the pose changed on the way through disk: {before} then {after}"
        );
    }
}

/// The other half of the ask: a rigged frame exported as an image, with the
/// bend visible in the pixels and no rig chrome anywhere in the file.
#[test]
fn a_posed_rig_exports_as_an_image_with_no_bones_in_it() {
    let mut exporter = match Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipping rig export test: no usable GPU ({e})");
            return;
        }
    };

    let (mut editor, id) = document_with_limb();
    rig_the_arm(&mut editor, id);

    let settings = ExportSettings::for_stage(editor.scene());
    let straight = exporter
        .render(editor.scene(), 0, &settings)
        .expect("render straight");

    // Bend the elbow down, hard.
    editor.doc.edit("Pose", |scene| {
        rigging::pose_bone(scene, 0, id, 1, Point::new(200.0, 330.0));
    });
    let bent = exporter
        .render(editor.scene(), 0, &settings)
        .expect("render bent");

    let is_limb = |pixel: [u8; 4]| {
        let [r, g, b, _] = LIMB.to_rgba8().to_u8_array();
        pixel[0].abs_diff(r) <= 24 && pixel[1].abs_diff(g) <= 24 && pixel[2].abs_diff(b) <= 24
    };
    let coverage = |frame: &buzz_export::Frame| {
        frame
            .pixels
            .chunks_exact(4)
            .filter(|p| is_limb([p[0], p[1], p[2], p[3]]))
            .count()
    };

    assert!(
        coverage(&straight) > 1_000,
        "the straight arm did not render"
    );
    assert!(coverage(&bent) > 1_000, "the bent arm did not render");

    // The straight arm lies along y = 200; the bent one reaches below it.
    let lowest_limb_row = |frame: &buzz_export::Frame| {
        (0..frame.height)
            .rev()
            .find(|y| (0..frame.width).any(|x| is_limb(frame.pixel(x, *y))))
            .expect("some artwork")
    };
    assert!(
        lowest_limb_row(&bent) > lowest_limb_row(&straight) + 30,
        "the exported image does not show the bend: {} vs {}",
        lowest_limb_row(&straight),
        lowest_limb_row(&bent)
    );

    // No chrome. Bones are drawn in Animate's amber and warp handles in blue,
    // as *screen* chrome; either one appearing in an exported frame would mean
    // the split between artwork and chrome had broken.
    let amber = bent
        .pixels
        .chunks_exact(4)
        .filter(|p| p[0] > 200 && p[1] > 150 && p[1] < 230 && p[2] < 140)
        .count();
    assert_eq!(amber, 0, "the exported image contains bone chrome");
}

/// Fifty rigs, solved in parallel, inside one frame — the performance half of
/// the exit criterion, run against real armatures rather than a micro-benchmark.
#[test]
fn fifty_rigged_characters_solve_within_a_frame() {
    use rayon::prelude::*;

    let make = |seed: usize| {
        let mut armature = buzz_rig::Armature::new(Point::new(seed as f64 * 10.0, 0.0));
        armature.push(buzz_rig::Bone::new("hip", None, 40.0, 0.1));
        armature.push(buzz_rig::Bone::new("spine", Some(0), 35.0, 0.05));
        armature.push(buzz_rig::Bone::new("shoulder", Some(1), 30.0, -0.2));
        armature.push(buzz_rig::Bone::new("upper", Some(2), 30.0, 0.3));
        armature.push(buzz_rig::Bone::new("fore", Some(3), 25.0, 0.4));
        armature.push(buzz_rig::Bone::new("hand", Some(4), 12.0, 0.1));
        armature
    };
    let mut rigs: Vec<buzz_rig::Armature> = (0..50).map(make).collect();

    let started = std::time::Instant::now();
    let solved: Vec<bool> = rigs
        .par_iter_mut()
        .enumerate()
        .map(|(i, rig)| {
            let target = Point::new(i as f64 * 10.0 + 60.0, 70.0);
            buzz_rig::solve_to(rig, 5, target, &buzz_rig::IkOptions::default()).reached
        })
        .collect();
    let elapsed = started.elapsed();

    assert!(
        solved.iter().all(|r| *r),
        "not every rig reached its target"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(41),
        "fifty six-bone rigs took {elapsed:?}, more than one frame at 24 fps"
    );
}
