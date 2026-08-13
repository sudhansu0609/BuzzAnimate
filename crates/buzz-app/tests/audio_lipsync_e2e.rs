//! The soundtrack, end to end: import, cue from inside nested symbols, lip
//! sync from the root audio, save and reopen.
//!
//! Each piece is tested on its own already. What this covers is the seam that
//! matters: audio placed on the *root* timeline, a character symbol on the
//! root, a head symbol inside that — and the requirement that the root audio
//! is heard, and can be synced against, from every one of those levels.

use std::sync::Arc;

use buzz_app::editor::Editor;
use buzz_audio::{LipSyncOptions, Viseme};
use buzz_geom::Affine;
use buzz_scene::{LayerKind, ObjectKind, SymbolKind};

/// A short line of "dialogue": two bursts with a pause between them, written
/// as a real WAV file so the whole import path is exercised.
fn dialogue_wav() -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut out, spec).expect("writer");
        let mut push = |seconds: f64, hz: f64, amplitude: f64| {
            let frames = (seconds * 44_100.0) as usize;
            for i in 0..frames {
                let t = i as f64 / 44_100.0;
                let v = (t * hz * std::f64::consts::TAU).sin() * amplitude;
                writer.write_sample((v * 30_000.0) as i16).expect("sample");
            }
        };
        push(0.5, 600.0, 0.8);
        push(0.3, 0.0, 0.0);
        push(0.5, 1_800.0, 0.8);
        writer.finalize().expect("finalize");
    }
    out.into_inner()
}

fn editor_with_dialogue() -> (Editor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("line.wav");
    std::fs::write(&path, dialogue_wav()).expect("write wav");

    let mut editor = Editor::default();
    // The audio layer, on the root timeline.
    let audio_layer = {
        let mut made = None;
        editor.doc.edit("Audio layer", |scene| {
            made = Some(scene.add_layer("Audio", LayerKind::Normal));
        });
        made.expect("a layer")
    };
    editor.selection.set_active_layer(Some(audio_layer));

    let name = editor.import_sound(&path).expect("the sound should import");
    assert_eq!(name, "line");
    editor.run(buzz_ui::Command::AttachSound);

    (editor, dir)
}

/// Build a character containing a head, both on the root, and return their ids.
fn character_with_head(editor: &mut Editor) -> (buzz_scene::SymbolId, buzz_scene::SymbolId) {
    let mut made = None;
    editor.doc.edit("Character", |scene| {
        let head = scene.add_symbol("Head", SymbolKind::MovieClip, None);
        let character = scene.add_symbol("Character", SymbolKind::MovieClip, None);

        let inner = scene
            .library()
            .get(character)
            .expect("the character")
            .layers
            .iter()
            .next()
            .expect("a layer")
            .id;
        scene.library_mut().update(character, |symbol| {
            symbol.layers.update(inner, |layer| {
                layer.frames.set_objects(
                    0,
                    vec![Arc::new(buzz_scene::Object {
                        id: buzz_scene::ObjectId(9100),
                        name: None,
                        transform: Affine::IDENTITY,
                        kind: ObjectKind::Instance(buzz_scene::SymbolInstance::new(head)),
                        locked: false,
                        visible: true,
                        filters: Vec::new(),
                        blend: Default::default(),
                        spatial: Default::default(),
                    })],
                );
            });
        });

        let stage_layer = scene.stage_layers().iter().next().expect("a layer").id;
        scene.add_instance_at(stage_layer, 0, character, Affine::IDENTITY);
        made = Some((character, head));
    });
    made.expect("built")
}

#[test]
fn a_sound_imports_attaches_and_is_cued() {
    let (editor, _dir) = editor_with_dialogue();

    assert_eq!(
        editor.scene().sounds().len(),
        1,
        "the sound is in the library"
    );
    let cues = editor.scene().stage_cues();
    assert_eq!(cues.len(), 1, "it should be cued from its keyframe");
    assert_eq!(cues[0].start_frame, 0);

    // And it decoded, so the timeline can draw its envelope.
    let waveforms = editor.waveforms();
    assert_eq!(waveforms.len(), 1, "one layer should have a waveform");
    let waveform = waveforms.values().next().expect("a waveform");
    assert!(
        waveform.levels.len() > 20,
        "1.3 seconds at 24 fps is about 32 frames, got {}",
        waveform.levels.len()
    );
    assert!(
        waveform.levels.iter().any(|l| *l > 0.1),
        "the envelope should show the speech"
    );
}

/// **The request.** Root audio stays cued — and stays the track lip sync will
/// use — from the root, from inside the character, and from inside its head.
#[test]
fn root_audio_is_available_at_every_level_of_nesting() {
    let (mut editor, _dir) = editor_with_dialogue();
    let (character, head) = character_with_head(&mut editor);

    let on_root = editor.scene().stage_cues();
    assert_eq!(on_root.len(), 1);

    // Inside the character.
    editor.doc.edit_view(|scene| {
        scene.enter_symbol(character);
    });
    assert_eq!(
        editor.scene().stage_cues(),
        on_root,
        "the root dialogue must still be cued inside the character"
    );
    let (track, _, _) = editor.lip_sync_choices();
    assert!(
        track.as_deref().is_some_and(|t| t.contains("line")),
        "the dialogue should be offered inside the character: {track:?}"
    );

    // Inside the head, one deeper.
    editor.doc.edit_view(|scene| {
        scene.enter_symbol(head);
    });
    assert_eq!(editor.scene().edit_path().len(), 2, "two symbols deep");
    assert_eq!(
        editor.scene().stage_cues(),
        on_root,
        "the root dialogue must still be cued inside the head"
    );
    let (track, _, layers) = editor.lip_sync_choices();
    assert!(
        track.as_deref().is_some_and(|t| t.contains("line")),
        "the dialogue should be offered two symbols deep: {track:?}"
    );
    assert!(
        !layers.is_empty(),
        "the head's own layers should be offered to put the mouth on"
    );
}

/// Lip sync run from *inside the head symbol*, against the *root* dialogue —
/// which is exactly how a character's mouth is animated.
#[test]
fn lip_sync_inside_the_head_symbol_uses_the_root_dialogue() {
    let (mut editor, _dir) = editor_with_dialogue();
    let (character, head) = character_with_head(&mut editor);

    let mouth = editor.new_mouth_symbol();

    editor.doc.edit_view(|scene| {
        scene.enter_symbol(character);
        scene.enter_symbol(head);
    });
    let mouth_layer = editor.scene().layers().iter().next().expect("a layer").id;

    editor.lip_sync = buzz_ui::LipSyncState::opened();
    editor.lip_sync.mouth = Some(mouth.0);
    editor.lip_sync.layer = Some(mouth_layer.0);
    editor.run_lip_sync();

    let result = editor.lip_sync.result.clone().expect("a result");
    assert!(
        result.contains("keyframes"),
        "lip sync did not run: {result}"
    );

    // The mouth was keyed inside the head, with instances of the mouth symbol.
    let layer = editor
        .scene()
        .layers()
        .get(mouth_layer)
        .expect("the layer")
        .clone();
    let keyed: Vec<u32> = layer
        .frames
        .keyframes()
        .iter()
        .filter(|k| !k.objects.is_empty())
        .map(|k| k.start)
        .collect();
    assert!(
        keyed.len() > 2,
        "expected several mouth keys, got {keyed:?}"
    );

    let mut shapes = std::collections::BTreeSet::new();
    for keyframe in layer.frames.keyframes() {
        for object in keyframe.objects.iter() {
            let ObjectKind::Instance(instance) = &object.kind else {
                panic!("expected a mouth instance");
            };
            assert_eq!(instance.symbol, mouth);
            shapes.insert(instance.first_frame);
        }
    }
    assert!(
        shapes.len() > 1,
        "the mouth should change shape across the line, saw {shapes:?}"
    );
    assert!(
        shapes.iter().all(|f| *f < Viseme::COUNT),
        "every shape must be a frame of the mouth symbol"
    );

    // The root timeline was not touched by any of it.
    editor.doc.edit_view(|scene| {
        scene.edit_document();
    });
    let root_audio = editor
        .scene()
        .layers()
        .iter()
        .find(|l| l.name == "Audio")
        .expect("the audio layer")
        .clone();
    assert!(
        root_audio.frames.keyframes()[0].sound.is_some(),
        "the dialogue should still be on the root"
    );
}

/// Sound survives the document: bytes in `media/`, references on keyframes.
#[test]
fn a_sound_survives_saving_and_reopening() {
    let (mut editor, dir) = editor_with_dialogue();
    let before = editor
        .scene()
        .sounds()
        .iter()
        .next()
        .expect("a sound")
        .clone();

    let path = dir.path().join("with-sound.buzz");
    editor.doc.save_as(&path).expect("save");

    let reopened = buzz_doc::Document::open(&path).expect("open");
    let after = reopened
        .scene()
        .sounds()
        .iter()
        .next()
        .expect("the sound should still be there")
        .clone();

    assert_eq!(after.name, before.name);
    assert_eq!(after.sample_rate, before.sample_rate);
    assert_eq!(after.length, before.length);
    assert_eq!(
        after.data.len(),
        before.data.len(),
        "the audio itself should have been stored in media/"
    );
    assert_eq!(
        reopened.scene().stage_cues().len(),
        1,
        "and the keyframe should still reference it"
    );

    // It decodes again from what was saved.
    let clip = buzz_audio::Clip::decode(&after.data, &after.name).expect("decode");
    assert!(clip.duration_seconds() > 1.0);
}

/// Analysis must produce mouth shapes that change over a line of dialogue —
/// silence closed, speech open — or lip sync is just noise.
#[test]
fn the_analysis_closes_the_mouth_in_the_pause() {
    let clip = buzz_audio::Clip::decode(&dialogue_wav(), "line").expect("decode");
    let track = buzz_audio::analyse_visemes(&clip, 24.0, &LipSyncOptions::default());

    // The pause runs from 0.5s to 0.8s, which is frames 12 to 19 at 24 fps.
    let closed_in_pause = track.frames[13..19]
        .iter()
        .filter(|v| v.is_closed())
        .count();
    assert!(
        closed_in_pause >= 3,
        "the pause should close the mouth: {:?}",
        track.frames
    );

    let open_in_speech = track.frames[..11].iter().filter(|v| !v.is_closed()).count();
    assert!(
        open_in_speech >= 5,
        "speech should open it: {:?}",
        track.frames
    );
}

/// Lip sync is one undo step, however many keyframes it writes.
#[test]
fn a_lip_sync_run_is_a_single_undo_step() {
    let (mut editor, _dir) = editor_with_dialogue();
    let mouth = editor.new_mouth_symbol();
    let layer = {
        let mut made = None;
        editor.doc.edit("Mouth layer", |scene| {
            made = Some(scene.add_layer("Mouth", LayerKind::Normal));
        });
        made.expect("a layer")
    };

    editor.lip_sync = buzz_ui::LipSyncState::opened();
    editor.lip_sync.mouth = Some(mouth.0);
    editor.lip_sync.layer = Some(layer.0);
    editor.run_lip_sync();

    let count_keys = |editor: &Editor| {
        editor
            .scene()
            .layers()
            .get(layer)
            .expect("the layer")
            .frames
            .keyframes()
            .iter()
            .filter(|k| !k.objects.is_empty())
            .count()
    };
    assert!(count_keys(&editor) > 2, "expected several keys");

    editor.run(buzz_ui::Command::Undo);
    assert_eq!(
        count_keys(&editor),
        0,
        "one undo should take back the whole run"
    );
}

/// A mouth symbol without a frame per shape is refused with both numbers, not
/// silently animated to the wrong shapes.
#[test]
fn a_short_mouth_symbol_is_refused_with_the_numbers() {
    let (mut editor, _dir) = editor_with_dialogue();
    let layer = editor.selection.active_layer().expect("a layer");

    let short = {
        let mut made = None;
        editor.doc.edit("Short symbol", |scene| {
            made = Some(scene.add_symbol("Too short", SymbolKind::Graphic, None));
        });
        made.expect("a symbol")
    };

    editor.lip_sync = buzz_ui::LipSyncState::opened();
    editor.lip_sync.mouth = Some(short.0);
    editor.lip_sync.layer = Some(layer.0);
    editor.run_lip_sync();

    let message = editor.lip_sync.result.clone().expect("a message");
    assert!(message.contains("needs"), "{message}");
    assert!(message.contains(&Viseme::COUNT.to_string()), "{message}");
}
