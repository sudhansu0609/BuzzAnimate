//! **A script and a casting file in, staged and captioned scenes out**, with no
//! audio and no window (`docs/SERIES_EP1_PLAN.md`, S4).
//!
//! The assets are made here, in a library of their own in a temp directory: a
//! rig with a mouth part standing in for the image pipeline's rigs, a prop and
//! a background. Nothing reads the user's library or the user's folders.

use std::sync::Arc;

use buzz_act::figure::{self, FigureSpec};
use buzz_act::screenplay::Screenplay;
use buzz_act::script_timing::Timing;
use buzz_app::episode::{BuildOptions, Casting, build_episode};
use buzz_doc::{AssetLibrary, Document};
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_scene::{LayerKind, Modifier, Object, ObjectId, ObjectKind, Scene, ShapeData};
use peniko::Color;

const SCRIPT: &str = "\
# EPISODE 1 \u{2014} TEST

---

## SCENE 4 \u{2014} THE THREE BEROZGAARS

**EXT. CHHOTU'S TEA STALL \u{2014} MORNING**

Tin roof.

Chhotu serves them.

**NARRATOR (V.O.)**
Guddu.

**GUDDU**
Main soch raha hoon kuch bada karun.

**CROW**
Kaun hai?

**SHARMA JI**
Dekho, gehun ka daam phir badha.

Beat.

---

## SCENE 5 \u{2014} RAJU ARRIVES

RAJU, 28, walks briskly toward them.

Guddu looks up.

**RAJU**
Aa gaya sarkar?

**GUDDU**
Haan.

Guddu leaves.

# END

## NOTES

Nothing here is a scene.
";

const CASTING: &str = r#"{
  "characters": {
    "GUDDU": {"cast": "Guddu"},
    "RAJU": {"cast": "Raju"},
    "CHHOTU": {"asset": "chaiwalNew"},
    "SHARMA JI": {"puppet": true},
    "NARRATOR": {"voice_only": true}
  },
  "scenes": {
    "4": {
      "background": ["backgroundRoadA"],
      "props": [{"asset": "chaiTapri", "x": 0.7, "y": 0.62, "h": 0.5}],
      "place": {"GUDDU": {"x": 0.3}, "SHARMA JI": {"x": 0.5}}
    },
    "5": {
      "setting": "daylight",
      "place": {"GUDDU": {"x": 0.3}, "RAJU": {"x": 0.7}}
    }
  }
}"#;

/// An eye like the image pipeline's: a composite of a lid, a pupil and a white,
/// the lid being a three-frame symbol shown one frame at a time.
fn eye_symbol(scene: &mut Scene, name: &str) -> buzz_scene::SymbolId {
    let ball = |r: f64, c: Color| {
        ShapeData::filled(kurbo::Ellipse::new((0.0, 0.0), (r, r * 0.7), 0.0).to_path(0.1), c)
    };
    let lid = scene.add_symbol(format!("{name}_eyelid"), buzz_scene::SymbolKind::Graphic, None);
    let lid_layer = scene.library().get(lid).unwrap().layers.iter().next().unwrap().id;
    let lids: Vec<Arc<Object>> = (0..3)
        .map(|i| {
            let id = scene.next_object_id();
            Arc::new(Object::shape(id, ball(4.0 + i as f64, Color::from_rgb8(0x20, 0x10, 0x10))))
        })
        .collect();
    scene.library_mut().update(lid, |s| {
        s.layers.update(lid_layer, |l| {
            while l.frames.length() < 3 {
                l.frames.insert_frame(l.frames.length());
            }
            for (f, o) in lids.iter().enumerate() {
                l.frames.insert_keyframe(f as u32);
                l.frames.set_objects(f as u32, vec![o.clone()]);
            }
        });
    });

    let eye = scene.add_symbol(format!("{name}_eye"), buzz_scene::SymbolKind::Graphic, None);
    let layer = scene.library().get(eye).unwrap().layers.iter().next().unwrap().id;
    let mut lid_instance = Object::instance_of(scene.next_object_id(), lid);
    if let ObjectKind::Instance(i) = &mut lid_instance.kind {
        i.loop_mode = buzz_scene::LoopMode::SingleFrame;
        i.first_frame = 0;
    }
    scene.library_mut().update(eye, |s| {
        s.layers.update(layer, |l| {
            l.name = "Eyelid".to_string();
            l.frames.set_objects(0, vec![Arc::new(lid_instance)]);
        });
    });
    eye
}

/// A rig like the image pipeline's: a figure whose mouth and eyes are bound parts.
fn rig_scene(name: &str) -> Scene {
    let mut scene = Scene::default();
    let layer = scene.add_layer(name, LayerKind::Normal);
    let id = scene.next_object_id();
    let mut rig = figure::build(&FigureSpec::default(), id, || scene.next_object_id());
    rig.name = Some(name.to_string());
    rig.transform = Affine::translate((150.0, 340.0));
    let symbol = buzz_act::lipsync::placeholder_mouth(&mut scene, &format!("{name}_mouth"));
    let mouth_id = scene.next_object_id();
    let mut mouth = Object::instance_of(mouth_id, symbol);
    mouth.transform = Affine::translate((0.0, -290.0));
    let eye = eye_symbol(&mut scene, name);
    let mut eyes = Vec::new();
    for x in [-20.0, 20.0] {
        let mut o = Object::instance_of(scene.next_object_id(), eye);
        o.transform = Affine::translate((x, -300.0));
        eyes.push(o);
    }
    if let ObjectKind::Armature(a) = &mut rig.kind {
        a.bind_rigid(Arc::new(mouth), 2);
        for e in eyes {
            a.bind_rigid(Arc::new(e), 2);
        }
    }
    scene.add_object(layer, rig).expect("the rig");
    scene
}

fn block_scene(name: &str, w: f64, h: f64) -> Scene {
    let mut scene = Scene::default();
    let layer = scene.add_layer(name, LayerKind::Normal);
    scene
        .add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 0.0, w, h).to_path(0.1), Color::from_rgb8(0x80, 0x60, 0x40)),
        )
        .expect("a block");
    scene
}

fn library(dir: &std::path::Path) -> AssetLibrary {
    let mut lib = AssetLibrary::at(dir);
    lib.save("Guddu", "Cast", &rig_scene("Guddu")).expect("Guddu");
    lib.save("Raju", "Cast", &rig_scene("Raju")).expect("Raju");
    lib.save("chaiTapri", "Animate/Objects", &block_scene("chaiTapri", 300.0, 200.0))
        .expect("the stall");
    lib.save("backgroundRoadA", "Animate/Objects", &block_scene("road", 3000.0, 1000.0))
        .expect("the road");
    lib.save("chaiwalNew", "Animate/Objects", &block_scene("chaiwal", 100.0, 300.0))
        .expect("the tea seller");
    lib
}

struct Built {
    scenes: Vec<Scene>,
    reports: Vec<buzz_app::episode::SceneReport>,
    stage: Rect,
}

fn build(numbers: &[u32]) -> Built {
    let dir = tempfile::tempdir().expect("temp dir");
    let lib = library(dir.path());
    let play = Screenplay::parse(SCRIPT);
    let casting = Casting::from_json(CASTING).expect("casting");
    let opts = BuildOptions::default();
    let (doc, reports) = build_episode(&play, &casting, &lib, numbers, &opts);
    let scenes = doc.film();
    let stage = scenes[0].stage().stage_rect();
    Built {
        scenes,
        reports,
        stage,
    }
}

fn armature_named(scene: &Scene, name: &str) -> Option<ObjectId> {
    scene.layers().iter().find_map(|l| {
        l.all_objects()
            .find(|o| matches!(o.kind, ObjectKind::Armature(_)) && o.name.as_deref() == Some(name))
            .map(|o| o.id)
    })
}

fn keyframes_of(scene: &Scene, id: ObjectId) -> Vec<u32> {
    let (layer, _) = scene.find_object(id).expect("object");
    scene
        .layers()
        .get(layer)
        .expect("layer")
        .frames
        .keyframes()
        .iter()
        .map(|k| k.start)
        .collect()
}

fn object_at(scene: &Scene, id: ObjectId, frame: u32) -> Arc<Object> {
    let (layer, _) = scene.find_object(id).expect("object");
    scene
        .layers()
        .get(layer)
        .expect("layer")
        .objects_at(frame)
        .iter()
        .find(|o| o.id == id)
        .cloned()
        .expect("the object on that frame")
}

#[test]
fn one_document_scene_per_script_scene_and_the_frames_are_the_summed_timing() {
    let built = build(&[4, 5]);
    assert_eq!(built.scenes.len(), 2);

    let play = Screenplay::parse(SCRIPT);
    for (i, number) in [4u32, 5].iter().enumerate() {
        let want = Timing::default().schedule(play.scene(*number).unwrap()).total_frames;
        assert_eq!(built.reports[i].frames, want, "scene {number}");
        assert_eq!(built.scenes[i].frame_count(), want, "scene {number}");
        // Every layer reaches the end.
        for layer in built.scenes[i].layers().iter() {
            assert!(layer.length() >= want, "{} is short in scene {number}", layer.name);
        }
    }
}

#[test]
fn the_notes_after_the_end_marker_are_not_a_scene() {
    let play = Screenplay::parse(SCRIPT);
    assert_eq!(play.scenes.len(), 2);
    let built = build(&[4, 5, 6]);
    // Scene 6 does not exist: said so, no document scene made for it.
    assert_eq!(built.scenes.len(), 2);
    assert!(built.reports[2].warnings[0].contains("no scene 6"));
}

#[test]
fn every_line_is_captioned_once_with_its_speaker() {
    let built = build(&[4]);
    let scene = &built.scenes[0];
    let subtitles = scene
        .layers()
        .iter()
        .find(|l| l.name == "Subtitles")
        .expect("a Subtitles layer");

    let mut shown: Vec<(u32, String, bool)> = Vec::new();
    for key in subtitles.frames.keyframes() {
        for object in key.objects.iter() {
            if let Some(text) = &object.text {
                shown.push((key.start, text.content.clone(), text.style.italic));
            }
        }
    }
    // Four lines: the narrator's, Guddu's, the crow's (uncast, still captioned)
    // and Sharma Ji's.
    assert_eq!(shown.len(), 4, "{shown:?}");
    assert_eq!(shown[0].1, "Guddu.", "a voice-over has no speaker: {shown:?}");
    assert!(shown[0].2, "a voice-over is italic");
    assert_eq!(shown[1].1, "GUDDU: Main soch raha hoon kuch bada karun.");
    assert!(!shown[1].2);
    assert_eq!(shown[2].1, "CROW: Kaun hai?");
    assert_eq!(shown[3].1, "SHARMA JI: Dekho, gehun ka daam phir badha.");
    // In order, one after another.
    assert!(shown.windows(2).all(|w| w[0].0 < w[1].0), "{shown:?}");
    assert_eq!(built.reports[0].captions, 4);

    // Each caption comes down again: the last frame of the scene has none.
    let last = built.scenes[0].frame_count() - 1;
    assert!(
        subtitles.objects_at(last).iter().all(|o| o.text.is_none()),
        "a caption is still up on the last frame"
    );
    // And the captions are keyed where the lines start in the timing.
    let play = Screenplay::parse(SCRIPT);
    let schedule = Timing::default().schedule(play.scene(4).unwrap());
    let first_line = play
        .scene(4)
        .unwrap()
        .items
        .iter()
        .position(|i| matches!(i, buzz_act::screenplay::Item::Dialogue(_)))
        .unwrap();
    assert_eq!(shown[0].0, schedule.slots[first_line].start);
}

#[test]
fn white_text_over_a_dark_outline() {
    let built = build(&[4]);
    let subtitles = built.scenes[0]
        .layers()
        .iter()
        .find(|l| l.name == "Subtitles")
        .unwrap()
        .clone();
    let key = subtitles
        .frames
        .keyframes()
        .iter()
        .find(|k| !k.objects.is_empty())
        .unwrap();
    assert_eq!(key.objects.len(), 2, "an outline under the words");
    let ObjectKind::Shape(outline) = &key.objects[0].kind else {
        panic!("outline")
    };
    let ObjectKind::Shape(words) = &key.objects[1].kind else {
        panic!("words")
    };
    assert!(outline.stroke.is_some(), "the outline is stroked");
    assert!(words.stroke.is_none());
    let colour = |f: &Option<buzz_scene::FillSpec>| match f {
        Some(spec) => match &spec.paint {
            buzz_scene::Paint::Solid(c) => Some(c.to_rgba8().to_u8_array()),
            _ => None,
        },
        None => None,
    };
    assert_eq!(colour(&words.fill), Some([255, 255, 255, 255]));
    let dark = colour(&outline.fill).unwrap();
    assert!(dark[0] < 40 && dark[1] < 40 && dark[2] < 40, "{dark:?}");
}

#[test]
fn an_uncast_name_is_skipped_and_reported_never_a_failure() {
    let built = build(&[4]);
    let report = &built.reports[0];
    assert!(
        report.skipped.iter().any(|s| s.starts_with("CROW") && s.contains("captioned")),
        "{:?}",
        report.skipped
    );
    assert!(!report.cast.iter().any(|c| c.name == "CROW"));
    // Chhotu is mentioned in the action but the narrator is voice-only.
    assert!(report.cast.iter().any(|c| c.name == "CHHOTU"));
    assert!(!report.cast.iter().any(|c| c.name == "NARRATOR"));
}

#[test]
fn cast_stands_on_the_ground_line_at_a_common_height() {
    let built = build(&[4]);
    let scene = &built.scenes[0];
    let stage = built.stage;
    let guddu = armature_named(scene, "Guddu").expect("Guddu is on stage");
    let object = object_at(scene, guddu, 0);
    let bounds = scene.resolved_bounds(&object);

    let ground = stage.y0 + stage.height() * 0.9;
    assert!(
        (bounds.y1 - ground).abs() < 12.0,
        "the feet are at {}, not on the ground line {ground}",
        bounds.y1
    );
    let want = stage.height() * 0.45;
    assert!(
        (bounds.height() - want).abs() < 14.0,
        "the figure is {} tall, wanted {want}",
        bounds.height()
    );
    // At the x the casting file gave.
    let x = bounds.center().x;
    assert!((x - stage.width() * 0.3).abs() < 30.0, "stands at {x}");
}

#[test]
fn the_speaker_performs_and_everyone_else_idles_for_the_whole_scene() {
    let built = build(&[4]);
    let scene = &built.scenes[0];
    let guddu = armature_named(scene, "Guddu").expect("Guddu");
    let keys = keyframes_of(scene, guddu);
    // Keys on twos through the whole scene: far more than the one it started
    // with, and reaching the last second.
    assert!(keys.len() > 20, "{} keys", keys.len());
    assert!(
        *keys.last().unwrap() + 24 >= built.scenes[0].frame_count(),
        "performance stops at {}",
        keys.last().unwrap()
    );
}

#[test]
fn a_rigs_mouth_follows_the_words_and_is_closed_between_lines() {
    let built = build(&[4]);
    let scene = &built.scenes[0];
    let guddu = armature_named(scene, "Guddu").expect("Guddu");
    let (layer_id, _) = scene.find_object(guddu).unwrap();
    let layer = scene.layers().get(layer_id).unwrap();

    let mut seen = std::collections::BTreeSet::new();
    let mut first_open = None;
    for key in layer.frames.keyframes() {
        let obj = key.objects.iter().find(|o| o.id == guddu).expect("the rig");
        let ObjectKind::Armature(rig) = &obj.kind else {
            panic!()
        };
        let part = rig
            .parts
            .iter()
            .find_map(|p| match &p.artwork.kind {
                ObjectKind::Instance(i) if i.loop_mode == buzz_scene::LoopMode::SingleFrame => Some(i.first_frame),
                _ => None,
            })
            .expect("a single-frame mouth part on every key");
        seen.insert(part);
        if part != 0 && first_open.is_none() {
            first_open = Some(key.start);
        }
    }
    assert!(seen.len() >= 3, "the mouth should change shape: {seen:?}");
    assert!(seen.contains(&0), "and close");

    // The mouth is first open when Guddu's line begins, not before.
    let play = Screenplay::parse(SCRIPT);
    let sc = play.scene(4).unwrap();
    let schedule = Timing::default().schedule(sc);
    let line = sc
        .items
        .iter()
        .position(|i| matches!(i, buzz_act::screenplay::Item::Dialogue(d) if d.speaker == "GUDDU"))
        .unwrap();
    let start = schedule.slots[line].start;
    let opened = first_open.expect("the mouth opens");
    assert!(
        opened >= start.saturating_sub(1) && opened <= start + 6,
        "the mouth opened at {opened}, the line starts at {start}"
    );
    assert!(built.reports[0].cast.iter().any(|c| c.name == "GUDDU" && c.lip_synced));
}

#[test]
fn the_puppet_gets_a_mouth_layer_and_blinks() {
    let built = build(&[4]);
    let scene = &built.scenes[0];
    // The mouth layer is written by the audio lip sync's own writer: instances of
    // the Mouth symbol showing frames.
    let mouth = scene
        .layers()
        .iter()
        .find(|l| l.name == "SHARMA JI Mouth")
        .expect("the puppet's mouth layer");
    let mut frames = std::collections::BTreeSet::new();
    for key in mouth.frames.keyframes() {
        for o in key.objects.iter() {
            if let ObjectKind::Instance(i) = &o.kind {
                frames.insert(i.first_frame);
            }
        }
    }
    assert!(frames.len() >= 3, "{frames:?}");

    let eyes = scene
        .layers()
        .iter()
        .find(|l| l.name == "SHARMA JI Face")
        .and_then(|l| l.all_objects().find(|o| o.name.as_deref() == Some("SHARMA JI Eyes")).cloned())
        .expect("the puppet's eyes");
    assert!(
        eyes.modifiers.iter().any(|m| matches!(m, Modifier::Blink { .. })),
        "a Blink modifier on the eyes"
    );
}

#[test]
fn the_background_covers_the_stage_and_the_prop_has_its_height_and_base() {
    let built = build(&[4]);
    let scene = &built.scenes[0];
    let stage = built.stage;

    let union_of = |name: &str| -> Rect {
        let layer = scene.layers().iter().find(|l| l.name == name).expect("a layer");
        layer
            .objects_at(0)
            .iter()
            .map(|o| scene.resolved_bounds(o))
            .reduce(|a, b| a.union(b))
            .unwrap()
    };
    let road = union_of("road");
    assert!(road.x0 <= stage.x0 + 1.0 && road.x1 >= stage.x1 - 1.0, "{road:?}");
    assert!(road.y0 <= stage.y0 + 1.0 && road.y1 >= stage.y1 - 1.0, "{road:?}");

    let stall = union_of("chaiTapri");
    assert!((stall.height() - stage.height() * 0.5).abs() < 1.0, "{stall:?}");
    assert!((stall.y1 - stage.height() * 0.62).abs() < 1.0, "{stall:?}");
    assert!((stall.center().x - stage.width() * 0.7).abs() < 1.0, "{stall:?}");

    // Behind the cast: the road is under the stall is under the cast, in stack
    // order from the back.
    let order: Vec<&str> = scene.layers().iter().map(|l| l.name.as_str()).collect();
    let at = |n: &str| order.iter().position(|x| *x == n).unwrap();
    // The stack lists front first.
    assert!(at("Subtitles") < at("Guddu"));
    assert!(at("Guddu") < at("chaiTapri"));
    assert!(at("chaiTapri") < at("road"));
}

#[test]
fn an_arrival_enters_from_the_nearer_side_and_a_departure_leaves() {
    let built = build(&[5]);
    let scene = &built.scenes[0];
    let stage = built.stage;
    let raju = armature_named(scene, "Raju").expect("Raju");

    // On the right (x = 0.7), so he starts in the wings on the right.
    let first = scene.resolved_bounds(&object_at(scene, raju, 0));
    assert!(
        first.x0 > stage.x1,
        "Raju should start off stage right, not at {first:?}"
    );

    // And has walked in by the time he speaks.
    let play = Screenplay::parse(SCRIPT);
    let sc = play.scene(5).unwrap();
    let schedule = Timing::default().schedule(sc);
    let line = sc
        .items
        .iter()
        .position(|i| matches!(i, buzz_act::screenplay::Item::Dialogue(d) if d.speaker == "RAJU"))
        .unwrap();
    let at = schedule.slots[line].start + 2;
    let there = scene.resolved_bounds(&object_at(scene, raju, at.min(scene.frame_count() - 1)));
    assert!(
        (there.center().x - stage.width() * 0.7).abs() < 40.0,
        "Raju should be on his mark by his line: {there:?}"
    );

    // Guddu leaves at the end, to the nearer (left) wing, and stays gone.
    let guddu = armature_named(scene, "Guddu").expect("Guddu");
    let last = scene.frame_count() - 1;
    let gone = scene.resolved_bounds(&object_at(scene, guddu, last));
    assert!(gone.x1 < stage.x0, "Guddu should have left stage left: {gone:?}");

    let notes = &built.reports[0].cast;
    assert!(notes.iter().find(|c| c.name == "RAJU").unwrap().enters);
    assert!(notes.iter().find(|c| c.name == "GUDDU").unwrap().exits);
    // Raju was never meant to leave.
    assert!(!notes.iter().find(|c| c.name == "RAJU").unwrap().exits);
}

#[test]
fn building_a_scene_is_one_undo_step() {
    let dir = tempfile::tempdir().expect("temp dir");
    let lib = library(dir.path());
    let play = Screenplay::parse(SCRIPT);
    let casting = Casting::from_json(CASTING).expect("casting");
    let (mut doc, _) = build_episode(&play, &casting, &lib, &[4], &BuildOptions::default());
    let layers = doc.scene().layers().len();
    assert!(layers > 5);
    // The scene is one step: one undo takes all of it back, to the empty scene
    // a new document starts with.
    let fresh = Document::default().scene().layers().len();
    doc.undo();
    assert_eq!(doc.scene().layers().len(), fresh, "one undo took the whole scene back");
}

#[test]
fn an_unknown_asset_is_a_note_not_a_failure() {
    let dir = tempfile::tempdir().expect("temp dir");
    let lib = library(dir.path());
    let play = Screenplay::parse(SCRIPT);
    let casting = Casting::from_json(
        r#"{"characters": {"GUDDU": {"cast": "Nobody"}}, "scenes": {"4": {"background": ["noSuchRoad"], "props": [{"asset": "noSuchProp"}], "place": {"GUDDU": {"x": 0.3}}}}}"#,
    )
    .expect("casting");
    let (doc, reports) = build_episode(&play, &casting, &lib, &[4], &BuildOptions::default());
    assert_eq!(doc.film().len(), 1);
    let skipped = reports[0].skipped.join("\n");
    assert!(skipped.contains("noSuchRoad"), "{skipped}");
    assert!(skipped.contains("noSuchProp"), "{skipped}");
    assert!(skipped.contains("Nobody"), "{skipped}");
    // The captions are there regardless.
    assert!(reports[0].captions > 0);
}

#[test]
fn a_set_with_no_background_is_built_from_the_setting() {
    let built = build(&[5]);
    let names: Vec<&str> = built.scenes[0].layers().iter().map(|l| l.name.as_str()).collect();
    assert!(names.contains(&"Sky"), "{names:?}");
    assert!(built.reports[0].set.contains("Daylight"), "{}", built.reports[0].set);
}

#[test]
fn the_casting_file_is_read_in_any_case_and_tolerates_extras() {
    let casting = Casting::from_json(
        r#"{"characters": {"Guddu": {"cast": "Guddu", "something_new": 1}}, "scenes": {"4": {"place": {"guddu": {"x": 0.3}}}}, "notes": "x"}"#,
    )
    .expect("casting");
    assert!(casting.characters.contains_key("GUDDU"));
    assert!(casting.scene_set(4).place.contains_key("GUDDU"));
    assert!(Casting::from_json("{ not json").is_err());
}

#[test]
fn the_report_says_what_happened() {
    let built = build(&[4, 5]);
    let text = buzz_app::episode::report_markdown("test", &built.reports, 24.0);
    assert!(text.contains("## Scene 4"));
    assert!(text.contains("## Scene 5"));
    assert!(text.contains("CROW"), "the skipped name is in the report");
    assert!(text.contains("lip-synced"));
}


/// The keys on which a rig's first eye part is not showing its open drawing, with
/// how many distinct eye drawings it showed in all.
fn shut_keys(scene: &Scene, name: &str) -> (Vec<u32>, usize, usize) {
    let body = armature_named(scene, name).expect("the rig");
    let (layer_id, _) = scene.find_object(body).unwrap();
    let layer = scene.layers().get(layer_id).unwrap();
    let mut per_key: Vec<(u32, buzz_scene::SymbolId)> = Vec::new();
    for key in layer.frames.keyframes() {
        let obj = key.objects.iter().find(|o| o.id == body).expect("the rig");
        let ObjectKind::Armature(rig) = &obj.kind else {
            panic!()
        };
        let eye = rig
            .parts
            .iter()
            .find_map(|p| match &p.artwork.kind {
                ObjectKind::Instance(i)
                    if scene.library().get(i.symbol).is_some_and(|s| s.name.to_lowercase().contains("eye")) =>
                {
                    Some(i.symbol)
                }
                _ => None,
            })
            .expect("an eye part on every key");
        per_key.push((key.start, eye));
    }
    let open = per_key[0].1;
    let distinct: std::collections::BTreeSet<_> = per_key.iter().map(|(_, e)| *e).collect();
    let shut: Vec<u32> = per_key.iter().filter(|(_, e)| *e != open).map(|(f, _)| *f).collect();
    (shut, distinct.len(), per_key.len())
}

/// The rig's eyes are parts of its armature, where a Blink modifier would squash
/// the whole figure, so they blink by swapping the part's drawing for the
/// half-shut and shut eye, on the rig's own keys.
#[test]
fn a_rigs_eyes_blink_by_swapping_the_eye_parts_drawing() {
    let built = build(&[4]);
    let (shut, distinct, keys) = shut_keys(&built.scenes[0], "Guddu");
    assert_eq!(distinct, 3, "open, half and shut");
    assert!(shut.len() >= 2, "the eyes should shut on some keys");
    assert!(
        shut.len() * 4 < keys,
        "a character mostly has their eyes open: {} of {keys} keys",
        shut.len()
    );
    assert!(built.reports[0].cast.iter().find(|c| c.name == "GUDDU").unwrap().blinks);
}

#[test]
fn a_cast_does_not_blink_in_unison() {
    let built = build(&[5]);
    let (guddu, _, _) = shut_keys(&built.scenes[0], "Guddu");
    let (raju, _, _) = shut_keys(&built.scenes[0], "Raju");
    assert!(!guddu.is_empty() && !raju.is_empty());
    assert_ne!(guddu, raju, "two characters blinking on exactly the same frames");
}

/// A drawing that looks one way is mirrored to face the middle of the stage, and
/// is mirrored back by a walk the other way.
#[test]
fn a_drawing_that_looks_right_is_mirrored_to_face_the_middle() {
    let dir = tempfile::tempdir().expect("temp dir");
    let lib = library(dir.path());
    let play = Screenplay::parse(SCRIPT);
    let casting = Casting::from_json(
        r#"{"characters": {"GUDDU": {"cast": "Guddu", "faces": "right"}, "RAJU": {"cast": "Raju", "faces": "right"}},
            "scenes": {"4": {"place": {"GUDDU": {"x": 0.2}, "RAJU": {"x": 0.8}}}}}"#,
    )
    .expect("casting");
    let (doc, _) = build_episode(&play, &casting, &lib, &[4], &BuildOptions::default());
    let scene = &doc.film()[0];
    let stage = scene.stage().stage_rect();
    let scale_x = |name: &str| {
        let id = armature_named(scene, name).expect("rig");
        object_at(scene, id, 0).transform.as_coeffs()[0]
    };
    // On the left, looking right: as drawn. On the right: mirrored to look left.
    assert!(scale_x("Guddu") > 0.0);
    assert!(scale_x("Raju") < 0.0, "Raju should face the middle");

    // Mirroring does not move the feet.
    let raju = armature_named(scene, "Raju").unwrap();
    let bounds = scene.resolved_bounds(&object_at(scene, raju, 0));
    assert!((bounds.center().x - stage.width() * 0.8).abs() < 30.0, "{bounds:?}");
    assert!((bounds.y1 - stage.height() * 0.9).abs() < 12.0, "{bounds:?}");
}

/// The built-in puppet stands on the right of the stage facing left.
#[test]
fn a_puppet_faces_the_middle_of_the_stage() {
    let dir = tempfile::tempdir().expect("temp dir");
    let lib = library(dir.path());
    let play = Screenplay::parse(SCRIPT);
    let casting = Casting::from_json(
        r#"{"characters": {"GUDDU": {"puppet": true}}, "scenes": {"4": {"place": {"GUDDU": {"x": 0.85}}}}}"#,
    )
    .expect("casting");
    let (doc, _) = build_episode(&play, &casting, &lib, &[4], &BuildOptions::default());
    let scene = &doc.film()[0];
    let body = armature_named(scene, "GUDDU").expect("the puppet");
    assert!(object_at(scene, body, 0).transform.as_coeffs()[0] < 0.0);
}

// -- LR1: a Cast member rigged by layer parenting ------------------------------------------

const LAYERED_SCRIPT: &str = "\
# EPISODE 1 \u{2014} TEST

## SCENE 1 \u{2014} LAYERS

**EXT. ROAD \u{2014} DAY**

Guddu arrives.

**GUDDU**
Arre suno, yeh bus kitne baje jaayegi, humko aaj hi bazaar tak pahunchna hai.

Guddu looks down the empty road for a long time, then back at the stall.

**GUDDU**
Theek hai, main yahin rukta hoon, tum jaao, main baad mein aata hoon.

The bus does not come. The wind picks up. Nobody says anything for a while.
";

/// `rig_scene`'s rig taken apart into a layer rig by Bones \u{2192} Layers: one
/// layer per part, named after its bone, the mouth and the eyes on head layers.
fn layered_rig_scene(name: &str) -> Scene {
    let mut scene = rig_scene(name);
    let rig = scene
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(0).iter())
        .find(|o| matches!(o.kind, ObjectKind::Armature(_)))
        .map(|o| o.id)
        .expect("the rig");
    buzz_app::rigging::bones_to_layers(&mut scene, 0, rig).expect("a layer rig");
    scene
}

/// The keyframes on the layer holding `id`.
fn keys_on_layer_of(scene: &Scene, id: ObjectId) -> Vec<u32> {
    keyframes_of(scene, id)
}

/// A Cast member whose asset is a layer rig is placed, enters, talks, waves,
/// idles, blinks and is lip-synced, all on its part layers, with no warning.
#[test]
fn a_layer_rig_cast_member_performs_blinks_and_is_lip_synced() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut lib = AssetLibrary::at(dir.path());
    lib.save("Guddu", "Cast", &layered_rig_scene("Guddu")).expect("Guddu");
    let play = Screenplay::parse(LAYERED_SCRIPT);
    let casting = Casting::from_json(r#"{"characters": {"GUDDU": {"cast": "Guddu"}}}"#).expect("casting");
    let (doc, reports) = build_episode(&play, &casting, &lib, &[1], &BuildOptions::default());
    let report = &reports[0];
    let scene = &doc.film()[0];

    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let note = report.cast.iter().find(|c| c.name == "GUDDU").expect("Guddu in the report");
    assert!(note.performs && note.lip_synced && note.blinks && note.enters, "{note:?}");
    assert_eq!(report.gestures.len(), 1, "{:?}", report.gestures);
    assert_eq!(report.gestures[0].action, buzz_act::Action::Wave);
    assert!(
        scene
            .layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .all(|o| !matches!(o.kind, ObjectKind::Armature(_))),
        "a layer rig holds no armature"
    );

    // The part layers, by their slot names.
    let part = |name: &str| -> ObjectId {
        scene
            .layers()
            .iter()
            .find(|l| l.name == name && l.follows.is_some())
            .and_then(|l| l.objects_at(0).first())
            .map(|o| o.id)
            .unwrap_or_else(|| panic!("no {name} layer"))
    };
    let elbow = part("Elbow R");
    let keys = keys_on_layer_of(scene, elbow);
    assert!(keys.len() > 20, "{} keys on Elbow R", keys.len());
    // Talk and the walk on twos, the idle on fours.
    assert!(keys.windows(2).any(|w| w[1] - w[0] == 2), "{keys:?}");
    assert!(keys.windows(2).any(|w| w[1] - w[0] == 4), "no idle keys: {keys:?}");
    // The wave's keys inside the line, as classic tweens.
    let wave = report.gestures[0].frame;
    assert!(keys.contains(&(wave + 16)), "{keys:?} has no key in the wave at {}", wave + 16);
    let (layer, _) = scene.find_object(elbow).unwrap();
    let tween = scene.layers().get(layer).unwrap().frames.tween_at(wave + 16);
    assert_eq!(tween.kind, buzz_scene::TweenKind::Classic);
    // Every part layer keyed alike.
    let root_keys = keys_on_layer_of(scene, part("Chest"));
    assert!(root_keys.len() > 20);

    // The eyes blink on their own layers: open, half and shut.
    let eyes: Vec<ObjectId> = scene
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(0).iter())
        .filter(|o| match &o.kind {
            ObjectKind::Instance(i) => scene
                .library()
                .get(i.symbol)
                .is_some_and(|s| s.name.to_lowercase().contains("eye")),
            _ => false,
        })
        .map(|o| o.id)
        .collect();
    assert_eq!(eyes.len(), 2, "two eye parts");
    let (eye_layer, _) = scene.find_object(eyes[0]).unwrap();
    let shown: std::collections::BTreeSet<_> = scene
        .layers()
        .get(eye_layer)
        .unwrap()
        .frames
        .keyframes()
        .iter()
        .filter_map(|k| match &k.objects.first()?.kind {
            ObjectKind::Instance(i) => Some(i.symbol),
            _ => None,
        })
        .collect();
    assert_eq!(shown.len(), 3, "open, half and shut");

    // The mouth, on its own layer, follows the words and closes.
    let mouth = scene
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(0).iter())
        .find(|o| match &o.kind {
            ObjectKind::Instance(i) => scene
                .library()
                .get(i.symbol)
                .is_some_and(|s| s.name.to_lowercase().contains("mouth")),
            _ => false,
        })
        .map(|o| o.id)
        .expect("the mouth part");
    let (mouth_layer, _) = scene.find_object(mouth).unwrap();
    let shapes: std::collections::BTreeSet<u32> = scene
        .layers()
        .get(mouth_layer)
        .unwrap()
        .frames
        .keyframes()
        .iter()
        .filter_map(|k| match &k.objects.first()?.kind {
            ObjectKind::Instance(i) if i.loop_mode == buzz_scene::LoopMode::SingleFrame => Some(i.first_frame),
            _ => None,
        })
        .collect();
    assert!(shapes.len() >= 3 && shapes.contains(&0), "{shapes:?}");
}
