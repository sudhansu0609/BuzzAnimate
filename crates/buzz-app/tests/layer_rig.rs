//! **A character rigged by layer parenting** — `rigging::rig_character_layers`
//! and the Bones ↔ Layers conversions, through the document, headless.

use buzz_app::{editor::Editor, rigging};
use buzz_geom::{Affine, Point, Rect, Shape as _};
use buzz_rig::RigPattern;
use buzz_scene::{LayerId, LayerKind, ObjectId, ObjectKind, ShapeData};
use peniko::Color;

const LIMB: Color = Color::from_rgb8(0xC8, 0x8A, 0x5A);

/// A chest, a left upper arm and a left forearm, drawn as bars on one layer:
/// the chest stands upright, the arm reaches out to the left of it.
fn three_parts() -> (Editor, RigPattern, Vec<Option<ObjectId>>) {
    let mut editor = Editor::default();
    let mut layer = None;
    editor.doc.edit("Layer", |scene| {
        layer = Some(scene.add_layer("Character", LayerKind::Normal));
    });
    let layer = layer.expect("a layer");

    let bars = [
        ("Chest", Rect::new(200.0, 0.0, 260.0, 120.0)),
        ("Shoulder L", Rect::new(140.0, 0.0, 200.0, 20.0)),
        ("Elbow L", Rect::new(80.0, 0.0, 140.0, 20.0)),
    ];
    let pattern = RigPattern::biped();
    let mut slots = vec![None; pattern.slots.len()];
    for (slot, bar) in bars {
        let mut id = None;
        editor.doc.edit("Draw", |scene| {
            id = scene.add_shape(layer, ShapeData::filled(bar.to_path(1e-9), LIMB));
        });
        slots[pattern.slot_named(slot).expect("a biped slot")] = id;
    }
    (editor, pattern, slots)
}

fn layer_of(rig: &rigging::LayerRig, pattern: &RigPattern, name: &str) -> LayerId {
    let slot = pattern.slot_named(name).expect("a slot");
    rig.layers
        .iter()
        .find(|(s, _)| *s == slot)
        .map(|(_, l)| *l)
        .expect("a layer for the slot")
}

fn rotate_about(centre: Point, degrees: f64) -> Affine {
    Affine::translate(centre.to_vec2())
        * Affine::rotate(degrees.to_radians())
        * Affine::translate(-centre.to_vec2())
}

/// Turning the upper arm 30° about its shoulder on frame 10 carries the
/// forearm with it: the forearm layer's inherited transform takes the elbow
/// to where the turned upper arm put it. The chest does not move.
#[test]
fn turning_the_upper_arm_carries_the_forearm_about_the_shoulder() {
    let (mut editor, pattern, slots) = three_parts();
    let mut rig = None;
    editor.doc.edit("Rig with Layers", |scene| {
        rig = rigging::rig_character_layers(scene, 0, &pattern, &slots);
    });
    let rig = rig.expect("a layer rig");
    assert_eq!(rig.layers.len(), 3);

    let chest = layer_of(&rig, &pattern, "Chest");
    let upper = layer_of(&rig, &pattern, "Shoulder L");
    let fore = layer_of(&rig, &pattern, "Elbow L");
    assert_eq!(rig.root, chest);

    let scene = editor.doc.scene();
    for (layer, name, follows) in [
        (chest, "Chest", None),
        (upper, "Shoulder L", Some(chest)),
        (fore, "Elbow L", Some(upper)),
    ] {
        let l = scene.layers().get(layer).expect("the layer");
        assert_eq!(l.name, name);
        assert_eq!(l.follows, follows, "{name} follows the wrong layer");
        assert_eq!(l.objects_at(0).len(), 1, "{name} is not alone on its layer");
    }

    // The joints the parts turn about, on the stage.
    let joint = |layer: LayerId| {
        let object = scene.layers().get(layer).unwrap().objects_at(0)[0].clone();
        (object.id, object.transform * object.pivot.expect("a pivot on the joint"))
    };
    let (arm_id, shoulder) = joint(upper);
    let (_, elbow) = joint(fore);
    let (chest_id, _) = joint(chest);
    assert!((shoulder - Point::new(200.0, 10.0)).hypot() < 0.5, "shoulder at {shoulder:?}");
    assert!((elbow - Point::new(140.0, 10.0)).hypot() < 0.5, "elbow at {elbow:?}");
    let chest_before = scene.find_object(chest_id).unwrap().1.transform;

    // Frame 10: a keyframe on the upper arm, turned 30° about its pivot.
    let turn = rotate_about(shoulder, 30.0);
    editor.doc.edit("Turn", |scene| {
        // F5 then F6: the layer is one frame long, and a keyframe beyond
        // the span would start empty.
        scene.update_layer(upper, |l| {
            l.frames.insert_frame(10);
            assert!(l.frames.insert_keyframe(10));
        });
        scene.update_object_at(10, arm_id, |o| o.transform = turn * o.transform);
    });

    let scene = editor.doc.scene();
    let moved = scene.layers().inherited_transform(fore, 10u32) * elbow;
    let wanted = turn * elbow;
    assert!(
        (moved - wanted).hypot() < 0.01,
        "the elbow went to {moved:?}, the turned arm put it at {wanted:?}"
    );
    assert!((moved - elbow).hypot() > 1.0, "the forearm did not move at all");

    // The chest is where it was, and passes nothing on.
    let identity = |a: Affine| {
        a.as_coeffs()
            .iter()
            .zip(Affine::IDENTITY.as_coeffs())
            .all(|(x, y)| (x - y).abs() < 1e-9)
    };
    assert!(identity(scene.layers().inherited_transform(upper, 10u32)));
    assert!(identity(scene.layers().motion_of(chest, 10u32)));
    assert_eq!(scene.find_object(chest_id).unwrap().1.transform, chest_before);
}

/// `(layer name, followed layer's name)` for every layer that follows another.
fn follow_names(scene: &buzz_scene::Scene) -> Vec<(String, String)> {
    let layers = scene.layers();
    let mut out: Vec<(String, String)> = layers
        .iter()
        .filter_map(|l| {
            let parent = layers.get(l.follows?)?;
            Some((l.name.clone(), parent.name.clone()))
        })
        .collect();
    out.sort();
    out
}

/// L3: a layer rig kept in the Cast is a scene with the parts on followed
/// layers and no armature. It saves and loads with its links and pivots, and
/// dropping it on the stage (moved to the drop point) keeps the links and
/// does not turn the move into motion the followers inherit.
#[test]
fn a_layer_rig_cast_asset_round_trips_and_drops_with_follows_intact() {
    let (mut editor, pattern, slots) = three_parts();
    editor.doc.edit("Rig with Layers", |scene| {
        rigging::rig_character_layers(scene, 0, &pattern, &slots).expect("a layer rig");
    });
    let wanted = follow_names(editor.doc.scene());
    assert_eq!(wanted.len(), 2, "{wanted:?}");

    let dir = tempfile::tempdir().expect("temp asset folder");
    let mut library = buzz_doc::AssetLibrary::at(dir.path());
    let asset = library
        .save("Layered", "Cast", editor.doc.scene())
        .expect("saved to the Cast");

    // Save → load.
    let loaded = library.load(&asset).expect("loads");
    assert_eq!(follow_names(&loaded), wanted, "the links did not survive the file");
    let armatures = loaded
        .layers()
        .iter()
        .flat_map(|l| l.all_objects())
        .filter(|o| matches!(o.kind, ObjectKind::Armature(_)))
        .count();
    assert_eq!(armatures, 0, "a layer rig holds no armature");
    let pivots = loaded
        .layers()
        .iter()
        .filter(|l| l.follows.is_some())
        .flat_map(|l| l.objects_at(0).iter())
        .filter(|o| o.pivot.is_some())
        .count();
    assert_eq!(pivots, 2, "the pivots did not survive the file");

    // Drop it into a fresh document, away from where it was drawn.
    let mut stage = Editor::default();
    stage.assets = library.clone();
    let depth = stage.doc.history().undo_depth();
    let drop = stage.camera.doc_to_screen(Point::new(600.0, 400.0));
    stage.place_asset_at(&asset, drop);
    assert_eq!(stage.doc.history().undo_depth(), depth + 1, "the drop is one undo step");
    let scene = stage.doc.scene();
    assert_eq!(follow_names(scene), wanted, "the drop lost the links");

    // Every part moved, and no follower inherits the move as motion.
    for layer in scene.layers().iter().filter(|l| l.follows.is_some()) {
        let inherited = scene.layers().inherited_transform(layer.id, 0u32);
        assert!(
            inherited
                .as_coeffs()
                .iter()
                .zip(Affine::IDENTITY.as_coeffs())
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "{} inherits {inherited:?} from the drop",
            layer.name
        );
        let object = &layer.objects_at(0)[0];
        let at = object.transform * object.pivot.expect("a pivot");
        assert!((at - Point::new(600.0, 400.0)).hypot() < 200.0, "{} at {at:?}", layer.name);
    }
}

/// Bones → Layers → Bones keeps every bone's name, length (within 0.5 px)
/// and rest angle (within 0.5°).
#[test]
fn bones_to_layers_and_back_keeps_the_skeleton() {
    let (mut editor, pattern, slots) = three_parts();
    let mut rig = None;
    editor.doc.edit("Rig", |scene| {
        rig = rigging::rig_character(scene, 0, &pattern, &slots);
    });
    let rig = rig.expect("a bone rig");
    let bones_of = |editor: &Editor, id: ObjectId| match &editor.doc.scene().find_object(id).unwrap().1.kind {
        ObjectKind::Armature(data) => data.armature.bones.clone(),
        other => panic!("not an armature: {other:?}"),
    };
    let before = bones_of(&editor, rig);

    let mut layers = None;
    editor.doc.edit("Convert to Layer Rig", |scene| {
        layers = rigging::bones_to_layers(scene, 0, rig);
    });
    let layers = layers.expect("a layer rig");
    assert_eq!(layers.layers.len(), 3);
    assert!(editor.doc.scene().find_object(rig).is_none(), "the armature is still there");
    for (slot, layer) in &layers.layers {
        let l = editor.doc.scene().layers().get(*layer).unwrap();
        assert_eq!(l.name, before[*slot].name);
    }

    let ids: Vec<LayerId> = layers.layers.iter().map(|(_, l)| *l).collect();
    let mut back = None;
    editor.doc.edit("Convert to Bone Rig", |scene| {
        back = rigging::layers_to_bones(scene, 0, &pattern, &ids);
    });
    let back = back.expect("a bone rig again");
    let after = bones_of(&editor, back);

    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(a.name, b.name);
        assert!((a.length - b.length).abs() < 0.5, "{}: {} vs {}", a.name, a.length, b.length);
        assert!(
            (a.rest_angle - b.rest_angle).abs().to_degrees() < 0.5,
            "{}: {} vs {}",
            a.name,
            a.rest_angle,
            b.rest_angle
        );
    }
    // The rig's own layers that emptied are gone; one holds the armature.
    let left = ids
        .iter()
        .filter(|id| editor.doc.scene().layers().get(**id).is_some())
        .count();
    assert_eq!(left, 1);
}

// ---------------------------------------------------------------------------
// LR1: the performances drive a layer rig as they drive the bone rig it came
// from.
// ---------------------------------------------------------------------------

/// A front-view person of seventeen bars, one per `biped_full` slot, the
/// character's left on the picture's right (as the Cast rigs hang it), rigged
/// with bones by `rig_character`. The joint limits are taken off: a layer rig
/// carries none, and the comparison is of the posing, not of the clamp.
fn person() -> (buzz_scene::Scene, ObjectId) {
    let mut scene = buzz_scene::Scene::default();
    scene.stage_mut().frame_rate = 24.0;
    let layer = scene.add_layer("Character", LayerKind::Normal);
    let pattern = RigPattern::biped_full();
    let bars = [
        ("Hips", Rect::new(185.0, 200.0, 215.0, 260.0)),
        ("Chest", Rect::new(180.0, 120.0, 220.0, 200.0)),
        ("Head", Rect::new(180.0, 60.0, 220.0, 120.0)),
        ("Shoulder L", Rect::new(222.0, 125.0, 236.0, 195.0)),
        ("Elbow L", Rect::new(222.0, 195.0, 236.0, 260.0)),
        ("Shoulder R", Rect::new(164.0, 125.0, 178.0, 195.0)),
        ("Elbow R", Rect::new(164.0, 195.0, 178.0, 260.0)),
        ("Thigh L", Rect::new(202.0, 260.0, 216.0, 340.0)),
        ("Knee L", Rect::new(202.0, 340.0, 216.0, 420.0)),
        ("Thigh R", Rect::new(184.0, 260.0, 198.0, 340.0)),
        ("Knee R", Rect::new(184.0, 340.0, 198.0, 420.0)),
        ("Wrist L", Rect::new(222.0, 260.0, 236.0, 285.0)),
        ("Wrist R", Rect::new(164.0, 260.0, 178.0, 285.0)),
        ("Ankle L", Rect::new(205.0, 420.0, 235.0, 432.0)),
        ("Ankle R", Rect::new(165.0, 420.0, 195.0, 432.0)),
        ("Brow L", Rect::new(205.0, 80.0, 215.0, 84.0)),
        ("Brow R", Rect::new(185.0, 80.0, 195.0, 84.0)),
    ];
    let mut slots = vec![None; pattern.slots.len()];
    for (name, bar) in bars {
        let id = scene.add_shape(layer, ShapeData::filled(bar.to_path(1e-9), LIMB));
        slots[pattern.slot_named(name).expect("a biped_full slot")] = id;
    }
    let rig = rigging::rig_character(&mut scene, 0, &pattern, &slots).expect("a bone rig");
    scene.update_object(rig, |o| {
        if let ObjectKind::Armature(data) = &mut o.kind {
            for bone in &mut data.armature.bones {
                bone.limits = None;
            }
        }
    });
    (scene, rig)
}

/// Every bound part's joint on the stage at `frame`, `(slot, point)`, as the
/// tweened armature puts its bone's head.
fn bone_joints(scene: &buzz_scene::Scene, rig: ObjectId, frame: u32) -> Vec<(usize, Point)> {
    let layer = scene.find_object(rig).expect("the rig").0;
    let resolved = scene.layers().get(layer).unwrap().frames.resolved_at(frame);
    let object = resolved.iter().find(|o| o.id == rig).expect("the rig at that frame");
    let ObjectKind::Armature(data) = &object.kind else {
        panic!("not an armature")
    };
    data.parts
        .iter()
        .map(|part| match part.binding {
            buzz_scene::RigBinding::Rigid(bone) => (bone, object.transform * data.armature.head(bone)),
            _ => panic!("a skinned part"),
        })
        .collect()
}

/// Every part's joint on the stage at `frame`, of the layer rig: its pivot
/// through its own tweened transform and what its layer inherits.
fn layer_joints(scene: &buzz_scene::Scene, rig: &rigging::LayerRig, frame: u32) -> Vec<(usize, Point)> {
    rig.layers
        .iter()
        .map(|(slot, layer)| {
            let resolved = scene.layers().get(*layer).unwrap().frames.resolved_at(frame);
            let object = resolved.iter().next().expect("the part at that frame");
            let inherited = scene.layers().inherited_transform(*layer, frame);
            (*slot, inherited * object.transform * object.pivot.expect("a pivot"))
        })
        .collect()
}

const LR1_ACTIONS: [buzz_act::Action; 5] = [
    buzz_act::Action::Walk,
    buzz_act::Action::Talk,
    buzz_act::Action::Wave,
    buzz_act::Action::Shrug,
    buzz_act::Action::Point,
];

/// Odd frames too: the in-betweens are the tweens', not the keys'.
const LR1_FRAMES: [u32; 5] = [0, 7, 16, 25, 34];

fn mirror_about(x: f64) -> Affine {
    Affine::translate((x, 0.0)) * Affine::scale_non_uniform(-1.0, 1.0) * Affine::translate((-x, 0.0))
}

fn compare_bone_and_layer(face_left: bool) {
    let (base, rig) = person();
    let mut layered = base.clone();
    let lrig = rigging::bones_to_layers(&mut layered, 0, rig).expect("a layer rig");
    assert_eq!(lrig.layers.len(), 17);
    let body = layered.layers().get(lrig.root).unwrap().objects_at(0)[0].id;
    match buzz_act::layer_rig::rig_of(&layered, body) {
        Some(buzz_act::layer_rig::CharacterRig::Layers(view)) => {
            assert_eq!(view.root, lrig.root);
            assert_eq!(view.parts.len(), 17);
        }
        other => panic!("the layer rig was not recognised: {other:?}"),
    }
    let at_rest = layer_joints(&layered, &lrig, 0);

    for action in LR1_ACTIONS {
        let mut bone = base.clone();
        let mut lay = layered.clone();
        let performance = buzz_act::Performance::new(action, 0..36);
        let mut placed_bone = buzz_act::layer_rig::placement(&bone, rig).unwrap();
        let mut placed_lay = buzz_act::layer_rig::placement(&lay, body).unwrap();
        if face_left {
            placed_bone = mirror_about(200.0) * placed_bone;
            placed_lay = mirror_about(200.0) * placed_lay;
        }
        buzz_act::perform_from(&mut bone, rig, &performance, placed_bone).expect("the bone rig performs");
        buzz_act::perform_from(&mut lay, body, &performance, placed_lay).expect("the layer rig performs");

        let mut worst = (0.0f64, String::new());
        for frame in LR1_FRAMES {
            let bones = bone_joints(&bone, rig, frame);
            let parts = layer_joints(&lay, &lrig, frame);
            assert_eq!(bones.len(), parts.len(), "one layer per part");
            for (slot, at) in &parts {
                let (_, want) = bones.iter().find(|(s, _)| s == slot).expect("the bone of that slot");
                let gap = (*at - *want).hypot();
                if gap > worst.0 {
                    worst = (gap, format!("slot {slot} at frame {frame}: layer {at:?}, bone {want:?}"));
                }
            }
        }
        assert!(worst.0 < 0.5, "{action:?} (facing left: {face_left}): {:.3} px apart, {}", worst.0, worst.1);

        // And it moved: this is not two rigs standing still.
        let moved = layer_joints(&lay, &lrig, 16)
            .iter()
            .zip(&at_rest)
            .map(|((_, a), (_, b))| (*a - *b).hypot())
            .fold(0.0, f64::max);
        assert!(moved > 1.0, "{action:?} did not move the layer rig");
    }
}

/// The same performance on the bone rig and on its layer version puts every
/// part's joint within half a pixel, at five frames, for Walk, Talk, Wave,
/// Shrug and Point.
#[test]
fn a_layer_rig_performs_exactly_as_the_bone_rig_it_came_from() {
    compare_bone_and_layer(false);
}

/// Facing left — the placement mirrored, as the episode builder turns a
/// character — the followers inherit the mirror and still agree.
#[test]
fn a_layer_rig_facing_left_performs_exactly_as_the_bone_rig() {
    compare_bone_and_layer(true);
}
