//! **Sequence: reusable setups, shots cut on the dialogue** — S4's commands,
//! checked through `Editor` the same way every other command is (headless,
//! no window, no PNG render). See `docs/SEQUENCE_SETUPS_PLAN.md` and
//! `docs/SEQUENCE_GUIDE.md`.

use buzz_app::editor::Editor;
use buzz_doc::SceneRole;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{LayerKind, ShapeData};
use peniko::Color;

/// A plain 4-second tone, written as a real WAV so `Set Master Audio` goes
/// through the whole import path rather than a back door. 4s at 24fps is
/// exactly 96 frames.
fn tone_wav(seconds: f64) -> Vec<u8> {
    let rate = 44_100u32;
    let wav = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut out, wav).expect("writer");
        let n = (seconds * rate as f64) as usize;
        for i in 0..n {
            let t = i as f64 / rate as f64;
            let v = (t * 440.0 * std::f64::consts::TAU).sin() * 0.4;
            writer.write_sample((v * 20_000.0) as i16).expect("sample");
        }
        writer.finalize().expect("finalize");
    }
    out.into_inner()
}

/// An editor with a 96-frame master audio set on "Scene 1" — the master
/// scene every test here builds its setups and shots beside.
fn editor_with_master_audio() -> (Editor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("dialogue.wav");
    std::fs::write(&path, tone_wav(4.0)).expect("write wav");

    let mut editor = Editor::default();
    editor
        .set_master_audio_from_file(&path)
        .expect("master audio sets");
    (editor, dir)
}

/// A distinctive mark on the active scene's first layer, so a shot's own
/// content can be told apart from a fresh stamp of its setup, or from
/// another setup entirely.
fn mark_scene(editor: &mut Editor, x: f64) {
    let layer = editor
        .doc
        .scene()
        .layers()
        .iter()
        .next()
        .expect("a layer")
        .id;
    editor.doc.edit("Mark", |scene| {
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(x, 0.0, x + 10.0, x + 10.0).to_path(1e-9), Color::WHITE),
        );
    });
    editor.doc.end_gesture();
}

/// Turn the active scene into a Setup and hand back its key.
fn make_setup(editor: &mut Editor) -> buzz_doc::SceneKey {
    editor.make_setup_from_scene();
    editor
        .doc
        .scene_key(editor.doc.active_scene())
        .expect("the active scene has a key")
}

/// **Sequence \u{25b8} New Shot from Setup** stamps a shot spanning the whole
/// master audio when the sequence is empty, and opens it for editing.
#[test]
fn new_shot_from_setup_stamps_a_shot_spanning_the_master_audio() {
    let (mut editor, _dir) = editor_with_master_audio();

    editor.add_scene();
    mark_scene(&mut editor, 100.0);
    let setup_key = make_setup(&mut editor);

    let shot_key = editor
        .new_shot_from_setup(setup_key)
        .expect("stamps a shot");
    let shot_index = editor.doc.index_of_key(shot_key).expect("the shot exists");

    match editor.doc.role(shot_index) {
        Some(SceneRole::Shot(info)) => {
            assert_eq!(info.from_setup, Some(setup_key));
            assert_eq!(info.audio_in, 0);
            assert_eq!(info.audio_out, 96);
        }
        other => panic!("expected a Shot, got {other:?}"),
    }
    assert_eq!(editor.doc.sequence(), &[shot_key]);
    assert_eq!(
        editor.doc.active_scene(),
        shot_index,
        "stamping opens the new shot for editing"
    );
}

/// **Split at Playhead**: the left shot keeps every layer it already has,
/// unchanged — only its `audio_out` shrinks — and the right half is a
/// *fresh* stamp of the same setup, not a copy of the left shot's own
/// edits.
#[test]
fn split_at_playhead_keeps_the_left_shots_layers_and_stamps_a_fresh_right_shot() {
    let (mut editor, _dir) = editor_with_master_audio();

    editor.add_scene();
    mark_scene(&mut editor, 100.0); // the setup's own mark
    let setup_key = make_setup(&mut editor);

    let shot_key = editor.new_shot_from_setup(setup_key).unwrap();
    mark_scene(&mut editor, 500.0); // the shot's own later edit

    let shot_index_before = editor.doc.index_of_key(shot_key).unwrap();
    let before = editor.doc.film()[shot_index_before].clone();

    editor.current_frame = 50;
    let right_key = editor
        .split_shot_at_playhead()
        .expect("splits the shot in two");

    assert_eq!(
        editor.doc.sequence(),
        &[shot_key, right_key],
        "the new shot sits right after the one it was split from"
    );

    // The left half: every layer but Dialogue is byte-identical to before
    // the split.
    let left_index = editor.doc.index_of_key(shot_key).unwrap();
    let left_after = &editor.doc.film()[left_index];
    for layer in before.stage_layers().iter() {
        if layer.name == "Dialogue" {
            continue;
        }
        let after_layer = left_after
            .stage_layers()
            .iter()
            .find(|l| l.id == layer.id)
            .expect("the layer is still there");
        assert_eq!(
            layer.frames, after_layer.frames,
            "layer \"{}\" changed by the split — the left half must keep every frame it had",
            layer.name
        );
    }
    match editor.doc.role(left_index) {
        Some(SceneRole::Shot(info)) => {
            assert_eq!(info.audio_in, 0);
            assert_eq!(info.audio_out, 50, "only audio_out shrinks");
        }
        other => panic!("expected the left half to stay a Shot: {other:?}"),
    }

    // The right half: a fresh stamp of the setup, spanning the rest of the
    // slice — not a copy of the left shot's own later edit.
    let right_index = editor.doc.index_of_key(right_key).unwrap();
    match editor.doc.role(right_index) {
        Some(SceneRole::Shot(info)) => {
            assert_eq!(info.from_setup, Some(setup_key));
            assert_eq!(info.audio_in, 50);
            assert_eq!(info.audio_out, 96);
        }
        other => panic!("expected the right half to be a Shot: {other:?}"),
    }
}

/// **Try Another Setup**: stamps a new take from a different setup and
/// swaps it into the cut — the old take is kept, not deleted.
#[test]
fn try_another_setup_stamps_a_new_take_and_keeps_the_old_one() {
    let (mut editor, _dir) = editor_with_master_audio();

    editor.add_scene();
    mark_scene(&mut editor, 100.0);
    let setup_a = make_setup(&mut editor);

    editor.add_scene();
    mark_scene(&mut editor, 200.0);
    let setup_b = make_setup(&mut editor);

    let original = editor.new_shot_from_setup(setup_a).unwrap();

    let take = editor
        .try_another_setup(original, setup_b)
        .expect("stamps a new take");

    assert!(
        editor.doc.index_of_key(original).is_some(),
        "the old take is kept, not deleted"
    );
    assert_eq!(
        editor.doc.sequence(),
        &[take],
        "the new take is in the cut in the original's place"
    );

    let take_index = editor.doc.index_of_key(take).unwrap();
    match editor.doc.role(take_index) {
        Some(SceneRole::Shot(info)) => {
            assert_eq!(info.from_setup, Some(setup_b));
            assert_eq!(info.take_of, Some(original));
        }
        other => panic!("expected a Shot: {other:?}"),
    }

    // Switching which take is in the cut is just `set_active_take` again.
    editor.doc.set_active_take(take, original);
    assert_eq!(editor.doc.sequence(), &[original]);
}

/// **Flatten to Master Scene** makes a new Normal scene and leaves the
/// sequence exactly as it was.
#[test]
fn flatten_to_master_scene_makes_a_new_normal_scene_and_leaves_the_sequence_unchanged() {
    let (mut editor, _dir) = editor_with_master_audio();

    editor.add_scene();
    mark_scene(&mut editor, 100.0);
    let setup_key = make_setup(&mut editor);

    editor.new_shot_from_setup(setup_key).unwrap();
    editor.current_frame = 40;
    editor.split_shot_at_playhead().unwrap();

    let sequence_before = editor.doc.sequence().to_vec();
    let scene_count_before = editor.doc.scene_names().len();

    let new_index = editor
        .flatten_to_master_scene()
        .expect("flattens the sequence");

    assert_eq!(editor.doc.scene_names().len(), scene_count_before + 1);
    assert_eq!(
        editor.doc.sequence(),
        sequence_before.as_slice(),
        "the sequence itself is unchanged"
    );
    match editor.doc.role(new_index) {
        Some(SceneRole::Normal) => {}
        other => panic!("expected a Normal scene: {other:?}"),
    }
    assert!(!editor.doc.film()[new_index].stage_layers().is_empty());
}

/// **Playback stops at the shot's own playing length**, not at its scene's
/// (possibly longer) `frame_count` — the frames a shortened shot keeps past
/// its new end are shown, greyed, but never played.
#[test]
fn playback_stops_at_the_shots_playing_length_not_the_scenes_frame_count() {
    let (mut editor, _dir) = editor_with_master_audio();

    editor.add_scene();
    mark_scene(&mut editor, 100.0);
    let setup_key = make_setup(&mut editor);
    let shot_key = editor.new_shot_from_setup(setup_key).unwrap();

    // Extend a layer well past where the shot will be shortened to, so the
    // scene's own frame_count outlives its playing length once shrunk.
    let layer = editor.doc.scene().layers().iter().next().unwrap().id;
    editor.doc.edit("Extend", |scene| {
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(90);
        });
    });
    editor.doc.end_gesture();

    editor.doc.set_shot_range(shot_key, 0, 40);
    assert!(
        editor.doc.scene().frame_count() > 40,
        "the shot should still be carrying its kept frames past the new end"
    );

    editor.playback.looping = false;
    editor.current_frame = 0;
    editor.playback.playing = true;
    editor.advance_playback(10.0); // far more than 40 frames at 24fps

    assert!(
        !editor.playback.playing,
        "playback should have stopped at the shot's own end"
    );
    assert_eq!(
        editor.current_frame, 39,
        "and rest on the shot's last playable frame, not run into the kept frames"
    );
}

// -- S7: Cut on Dialogue; per-shot lip-sync ----------------------------------

/// A three-line dialogue track — 1.5s, 1.0s, 2.0s of tone, separated and
/// bracketed by silence — the same shape `fit_to_narration.rs` uses, so
/// `detect_phrases` reliably finds three lines in it.
fn dialogue_wav() -> Vec<u8> {
    let rate = 44_100.0_f64;
    let spec: [(bool, f64); 7] = [
        (false, 0.3),
        (true, 1.5),
        (false, 0.5),
        (true, 1.0),
        (false, 0.6),
        (true, 2.0),
        (false, 0.3),
    ];
    let wav = hound::WavSpec {
        channels: 1,
        sample_rate: rate as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut out, wav).expect("writer");
        let mut at = 0.0_f64;
        for &(speaking, seconds) in &spec {
            let n = (seconds * rate) as usize;
            for _ in 0..n {
                let t = at / rate;
                let v = if speaking {
                    (t * 220.0 * std::f64::consts::TAU).sin() * 0.6
                        + (t * 660.0 * std::f64::consts::TAU).sin() * 0.2
                } else {
                    0.0
                };
                writer.write_sample((v * 28_000.0) as i16).expect("sample");
                at += 1.0;
            }
        }
        writer.finalize().expect("finalize");
    }
    out.into_inner()
}

/// An editor with the three-line dialogue track set as the master audio.
fn editor_with_dialogue() -> (Editor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("dialogue.wav");
    std::fs::write(&path, dialogue_wav()).expect("write wav");

    let mut editor = Editor::default();
    editor
        .set_master_audio_from_file(&path)
        .expect("master audio sets");
    (editor, dir)
}

/// **Cut on Dialogue never re-cuts an existing shot**, and the new shots it
/// stamps tile only the master's own uncovered stretches — the whole film
/// ends up covered exactly once, old shot and new alike.
#[test]
fn cut_on_dialogue_leaves_existing_shots_untouched_and_fills_only_uncovered_ranges() {
    let (mut editor, _dir) = editor_with_dialogue();

    editor.add_scene();
    mark_scene(&mut editor, 100.0);
    let setup = make_setup(&mut editor);

    // A shot already cut by hand over the middle of the film, before Cut on
    // Dialogue ever runs.
    let existing = editor.new_shot_from_setup(setup).unwrap();
    editor.doc.set_shot_range(existing, 60, 90);
    mark_scene(&mut editor, 500.0); // the existing shot's own later edit
    let existing_index = editor.doc.index_of_key(existing).unwrap();
    let before = editor.doc.film()[existing_index].clone();

    let total = editor.doc.master_audio_frames().expect("master audio frames");
    assert!(total > 90, "the test needs audio past the existing shot");

    let stamped = editor.cut_on_dialogue(setup);
    assert!(stamped > 0, "expected new shots: {:?}", editor.status);

    // The existing shot is untouched: every layer's frames are exactly what
    // they were before Cut on Dialogue ran.
    let existing_index_after = editor.doc.index_of_key(existing).unwrap();
    let after = &editor.doc.film()[existing_index_after];
    for layer in before.stage_layers().iter() {
        let after_layer = after
            .stage_layers()
            .iter()
            .find(|l| l.id == layer.id)
            .expect("the layer is still there");
        assert_eq!(
            layer.frames, after_layer.frames,
            "Cut on Dialogue touched the existing shot's \"{}\" layer",
            layer.name
        );
    }
    match editor.doc.role(existing_index_after) {
        Some(SceneRole::Shot(info)) => {
            assert_eq!((info.audio_in, info.audio_out), (60, 90), "the existing shot's own range moved");
        }
        other => panic!("expected a Shot: {other:?}"),
    }

    // Every shot in the cut, new or old, tiles the whole film with no gap
    // and no overlap.
    let mut ranges: Vec<(u32, u32)> = editor
        .doc
        .sequence()
        .to_vec()
        .iter()
        .filter_map(|k| editor.doc.index_of_key(*k))
        .filter_map(|i| match editor.doc.role(i) {
            Some(SceneRole::Shot(info)) => Some((info.audio_in, info.audio_out)),
            _ => None,
        })
        .collect();
    ranges.sort();
    assert_eq!(ranges[0].0, 0, "the film must be covered from frame 0: {ranges:?}");
    assert_eq!(
        ranges.last().unwrap().1,
        total,
        "the film must be covered to its own end: {ranges:?}"
    );
    for pair in ranges.windows(2) {
        assert_eq!(pair[0].1, pair[1].0, "a gap or an overlap: {ranges:?}");
    }
    assert!(
        ranges.contains(&(60, 90)),
        "the existing shot's own range must still be exactly what it was: {ranges:?}"
    );
}

/// **A captioned line goes to its speaker's own setup**, by name — everyone
/// else's lines go to the default setup handed to the command.
#[test]
fn cut_on_dialogue_sends_a_captioned_line_to_the_speakers_own_setup() {
    let (mut editor, _dir) = editor_with_dialogue();

    // Where the lines actually land, the same way `cut_on_dialogue` finds
    // out, so the caption below is placed exactly over the second one.
    let asset = editor.doc.master_sound().expect("master sound");
    let clip = buzz_audio::Clip::decode(&asset.data, &asset.name).expect("decode");
    let phrases = buzz_audio::detect_phrases(&clip, 24.0, &buzz_audio::PhraseOptions::default());
    assert_eq!(phrases.len(), 3, "expected three lines: {phrases:?}");
    let line1_start = phrases[0].start;
    let line2_start = phrases[1].start;

    editor.add_scene();
    mark_scene(&mut editor, 100.0);
    let default_setup = make_setup(&mut editor);

    editor.add_scene();
    editor.doc.rename_scene(editor.doc.active_scene(), "Ana CU");
    mark_scene(&mut editor, 200.0);
    let ana_setup = make_setup(&mut editor);

    // A Captions layer on the master scene (index 0), naming Ana for the
    // second line only.
    editor.doc.switch_scene(0);
    editor.doc.edit("Test Captions", |scene| {
        let layer = scene.add_layer("Captions", LayerKind::Normal);
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(140);
            l.frames.insert_blank_keyframe(line2_start);
            l.frames.set_label(line2_start, Some("Ana".to_string()));
            l.frames.insert_blank_keyframe(line2_start + 5);
        });
    });
    editor.doc.end_gesture();

    editor.cut_on_dialogue(default_setup);

    let ranges: Vec<(u32, u32, Option<buzz_doc::SceneKey>)> = editor
        .doc
        .sequence()
        .to_vec()
        .iter()
        .filter_map(|k| editor.doc.index_of_key(*k))
        .filter_map(|i| match editor.doc.role(i) {
            Some(SceneRole::Shot(info)) => Some((info.audio_in, info.audio_out, info.from_setup)),
            _ => None,
        })
        .collect();

    let line2_shot = ranges
        .iter()
        .find(|(s, e, _)| *s <= line2_start && line2_start < *e)
        .unwrap_or_else(|| panic!("no shot covers line 2 at frame {line2_start}: {ranges:?}"));
    assert_eq!(
        line2_shot.2,
        Some(ana_setup),
        "line 2 is captioned as Ana's, so it should go to Ana's setup: {ranges:?}"
    );

    let line1_shot = ranges
        .iter()
        .find(|(s, e, _)| *s <= line1_start && line1_start < *e)
        .unwrap_or_else(|| panic!("no shot covers line 1 at frame {line1_start}: {ranges:?}"));
    assert_eq!(
        line1_shot.2,
        Some(default_setup),
        "line 1 has no speaker on record, so it should go to the default setup: {ranges:?}"
    );
}

// -- S8: Continue from Shot ---------------------------------------------------

/// **Continue from Shot** seeds a still-untouched layer's frame 0 with the
/// source shot's last-frame pose, leaves a layer the user already animated
/// alone, and never changes the source.
#[test]
fn continue_from_shot_seeds_frame_zero_and_skips_a_layer_already_animated() {
    let (mut editor, _dir) = editor_with_master_audio();

    editor.add_scene();
    mark_scene(&mut editor, 100.0); // marks the scene's original layer
    let mut extra_layer = None;
    editor.doc.edit("Add Layer", |scene| {
        extra_layer = Some(scene.add_layer("Extra", LayerKind::Normal));
    });
    editor.doc.end_gesture();
    let extra_layer = extra_layer.unwrap();
    let setup = make_setup(&mut editor);

    let shot1 = editor.new_shot_from_setup(setup).unwrap();
    editor.doc.set_shot_range(shot1, 0, 40);
    let shot1_index = editor.doc.index_of_key(shot1).unwrap();
    let first_layer = editor.doc.film()[shot1_index]
        .stage_layers()
        .iter()
        .find(|l| l.name != "Extra" && l.name != "Dialogue")
        .expect("the setup's own layer")
        .id;

    // Shot 1's own performance, ending in a distinctive pose on both layers
    // at its own last playable frame (39).
    editor.doc.edit("Perform", |scene| {
        scene.update_layer(first_layer, |l| {
            l.frames.insert_keyframe(39);
        });
        scene.add_shape_at(
            first_layer,
            39,
            ShapeData::filled(Rect::new(700.0, 0.0, 710.0, 10.0).to_path(1e-9), Color::WHITE),
        );
        scene.update_layer(extra_layer, |l| {
            l.frames.insert_keyframe(39);
        });
        scene.add_shape_at(
            extra_layer,
            39,
            ShapeData::filled(Rect::new(800.0, 0.0, 810.0, 10.0).to_path(1e-9), Color::WHITE),
        );
    });
    editor.doc.end_gesture();
    let shot1_before = editor.doc.film()[shot1_index].clone();

    // Shot 2: a second, independent stamp of the same setup. Its "Extra"
    // layer is drawn on by the user before Continue from Shot ever runs;
    // its other layer is left exactly as it was stamped.
    let shot2 = editor.new_shot_from_setup(setup).unwrap();
    editor.doc.set_shot_range(shot2, 40, 80);
    let shot2_index = editor.doc.index_of_key(shot2).unwrap();
    editor.switch_scene(shot2_index);
    editor.doc.edit("User Draws", |scene| {
        scene.add_shape_at(
            extra_layer,
            0,
            ShapeData::filled(Rect::new(900.0, 0.0, 910.0, 10.0).to_path(1e-9), Color::WHITE),
        );
    });
    editor.doc.end_gesture();

    let (changed, skipped) = editor.continue_from_shot(shot2, shot1);

    assert_eq!(changed, 1, "only the untouched layer should change");
    assert_eq!(
        skipped,
        vec!["Extra".to_string()],
        "the layer the user already drew on should be reported, not touched"
    );

    // The source shot is completely unchanged.
    let shot1_index_after = editor.doc.index_of_key(shot1).unwrap();
    let shot1_after = &editor.doc.film()[shot1_index_after];
    for layer in shot1_before.stage_layers().iter() {
        let after_layer = shot1_after
            .stage_layers()
            .iter()
            .find(|l| l.id == layer.id)
            .expect("the layer is still there");
        assert_eq!(
            layer.frames, after_layer.frames,
            "Continue from Shot changed the source shot's \"{}\" layer",
            layer.name
        );
    }

    // Shot 2's continued layer now shows shot 1's last-frame pose at frame
    // 0 — and shot 2's "Extra" layer keeps the user's own drawing.
    let shot2_index_after = editor.doc.index_of_key(shot2).unwrap();
    let shot2_after = &editor.doc.film()[shot2_index_after];
    let shot2_first_layer = shot2_after
        .stage_layers()
        .iter()
        .find(|l| l.name != "Extra" && l.name != "Dialogue")
        .expect("the setup's own layer");
    let shot1_last_frame = editor.doc.playing_length(shot1_index_after).saturating_sub(1);
    let expected = shot1_after
        .stage_layers()
        .iter()
        .find(|l| l.id == first_layer)
        .expect("shot 1's own layer")
        .frames
        .frame_contents(shot1_last_frame)
        .unwrap_or_default();
    assert_eq!(
        shot2_first_layer.frames.frame_contents(0).unwrap_or_default(),
        expected,
        "shot 2's frame 0 should match shot 1's last-frame pose"
    );

    let shot2_extra = shot2_after
        .stage_layers()
        .iter()
        .find(|l| l.name == "Extra")
        .expect("the Extra layer");
    assert_eq!(
        shot2_extra.frames.frame_contents(0).unwrap().len(),
        1,
        "the user's own drawing on Extra must be left exactly as it was"
    );

    match editor.doc.role(shot2_index_after) {
        Some(SceneRole::Shot(info)) => {
            assert_eq!(info.continue_from, Some(shot1), "continue_from should be recorded");
        }
        other => panic!("expected a Shot: {other:?}"),
    }
}

// -- S7b: lip sync honours a shot's own trim ---------------------------------

/// A synthetic clip: silence, then a tone — so a phrase detector has exactly
/// one thing to find, and where it finds it says whether the trim in front
/// of it was honoured.
fn silence_then_tone_wav(silence_seconds: f64, tone_seconds: f64) -> Vec<u8> {
    let rate = 44_100u32;
    let wav = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut out, wav).expect("writer");
        let silence_n = (silence_seconds * rate as f64) as usize;
        for _ in 0..silence_n {
            writer.write_sample(0i16).expect("sample");
        }
        let tone_n = (tone_seconds * rate as f64) as usize;
        for i in 0..tone_n {
            let t = i as f64 / rate as f64;
            let v = (t * 440.0 * std::f64::consts::TAU).sin() * 0.6;
            writer.write_sample((v * 20_000.0) as i16).expect("sample");
        }
        writer.finalize().expect("finalize");
    }
    out.into_inner()
}

/// **Lip sync on a shot honours its own trim.** The master audio is 2s of
/// silence then 2s of tone (96 frames at 24fps); the shot is trimmed to
/// start 1s in (frame 24) — inside the silence, well before the tone at
/// frame 48. The shot's own Dialogue layer plays only `24..96` of the
/// master, so the tone starts at the shot's own frame 24 (`48 - 24`), and
/// that is where the first mouth change should land — not at frame 48,
/// which is where it would land if the trim were ignored and the whole
/// master clip were searched from its own front.
#[test]
fn lip_sync_on_a_trimmed_shot_keys_the_first_mouth_change_at_the_shots_own_frame() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("dialogue.wav");
    std::fs::write(&path, silence_then_tone_wav(2.0, 2.0)).expect("write wav");

    let mut editor = Editor::default();
    editor
        .set_master_audio_from_file(&path)
        .expect("master audio sets");

    editor.add_scene();
    mark_scene(&mut editor, 100.0);
    let setup = make_setup(&mut editor);

    let shot = editor.new_shot_from_setup(setup).unwrap();
    // Trim the shot to start 1s into the master (frame 24) — still inside
    // the silence, ahead of the tone at frame 48.
    editor.doc.set_shot_range(shot, 24, 96);

    let mouth = editor.new_mouth_symbol();
    let layer = editor
        .doc
        .scene()
        .stage_layers()
        .iter()
        .find(|l| l.name != "Dialogue")
        .expect("the setup's own layer")
        .id;

    editor.lip_sync.mouth = Some(mouth.0);
    editor.lip_sync.layer = Some(layer.0);
    editor.lip_sync.silence = 0.06;
    editor.lip_sync.hold = 2;

    let scene = editor.doc.scene().clone();
    editor.sound.refresh(&scene);

    editor.run_lip_sync();

    let target = editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .expect("the layer")
        .clone();
    let starts: Vec<u32> = target.frames.keyframes().iter().map(|k| k.start).collect();
    assert!(
        starts.len() >= 2,
        "expected at least a silent run and a loud one: {starts:?} \u{2014} {:?}",
        editor.lip_sync.result
    );
    let first_change = starts[1];
    assert!(
        (first_change as i64 - 24).abs() <= 4,
        "the mouth should change around the shot's own frame 24, not {first_change} \u{2014} \
         starts were {starts:?}"
    );
    assert!(
        first_change < 40,
        "frame {first_change} looks like the trim was ignored (the tone starts at master \
         frame 48, shot frame 48 untrimmed) rather than honoured (shot frame 24)"
    );
}
