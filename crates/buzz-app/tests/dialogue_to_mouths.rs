//! **A conversation, lip-synced from a subtitle file.**
//!
//! Lip sync could already turn a soundtrack into mouth shapes — but only one
//! mouth against the whole track, so running it on Ana had her mouthing Ben's
//! lines too. Fine for a monologue, useless for a conversation, which is most
//! of what a story is.
//!
//! The missing piece was never the analysis. It was knowing *who is speaking
//! and when*, and a subtitle file says exactly that. What is tested here is the
//! join: two named speakers, two mouths, and each one animated **only over
//! their own lines**.

use buzz_app::editor::Editor;
use buzz_scene::{LayerId, ObjectKind};
use buzz_ui::Command;

/// Two speakers, alternating, each with two lines so both are recognised as
/// characters rather than one-off prose.
const SRT: &str = "\
1
00:00:00,500 --> 00:00:02,000
Ana: We should go before it gets dark.

2
00:00:02,500 --> 00:00:04,000
Ben: I am not going anywhere.

3
00:00:04,500 --> 00:00:06,000
Ana: Then I will go on my own.

4
00:00:06,500 --> 00:00:08,000
Ben: Wait for me.
";

/// A dialogue recording: sound throughout, so every line has audio under it.
fn dialogue_wav(seconds: f64) -> Vec<u8> {
    let rate = 44_100.0_f64;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut out, spec).expect("writer");
        let n = (seconds * rate) as usize;
        for i in 0..n {
            let t = i as f64 / rate;
            // Two partials that wander, so the analysis sees changing shapes
            // rather than one held vowel for eight seconds.
            let v = (t * 180.0 * std::f64::consts::TAU).sin() * 0.5
                + (t * (700.0 + 400.0 * (t * 3.0).sin()) * std::f64::consts::TAU).sin() * 0.3;
            writer.write_sample((v * 26_000.0) as i16).expect("sample");
        }
        writer.finalize().expect("finalize");
    }
    out.into_inner()
}

/// An editor with the dialogue imported, captions on their own layer, and a
/// mouth symbol named for each speaker.
fn conversation() -> (Editor, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");

    let wav = dir.path().join("vo.wav");
    std::fs::write(&wav, dialogue_wav(9.0)).expect("write wav");
    let srt = dir.path().join("vo.srt");
    std::fs::write(&srt, SRT).expect("write srt");

    let mut editor = Editor::default();
    editor.import_sound(&wav).expect("the dialogue imports");

    // A mouth for each character, named after them — which is how they are
    // matched, and the whole of the setup this feature asks for.
    editor.doc.edit("Mouths", |scene| {
        buzz_app::lipsync::placeholder_mouth(scene, "Ana");
        buzz_app::lipsync::placeholder_mouth(scene, "Ben");
    });

    editor.import_captions(&srt).expect("the captions import");
    (editor, dir)
}

fn layer_named(editor: &Editor, name: &str) -> Option<LayerId> {
    editor
        .doc
        .scene()
        .layers()
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(name))
        .map(|l| l.id)
}

/// The frames on `layer` that carry a mouth instance.
fn mouth_frames(editor: &Editor, layer: LayerId) -> Vec<u32> {
    let scene = editor.doc.scene();
    let Some(l) = scene.layers().get(layer) else {
        return Vec::new();
    };
    let starts: Vec<u32> = l.frames.keyframes().iter().map(|k| k.start).collect();
    starts
        .into_iter()
        .filter(|at| {
            l.frames
                .resolved_at(*at)
                .iter()
                .any(|o| matches!(o.kind, ObjectKind::Instance(_)))
        })
        .collect()
}

/// **Two speakers get two mouths**, on layers of their own names.
#[test]
fn each_speaker_gets_their_own_mouth() {
    let (mut editor, _dir) = conversation();
    let written = editor
        .lip_sync_from_captions()
        .expect("the conversation should sync");
    assert!(written > 0, "{:?}", editor.status);

    let ana = layer_named(&editor, "Ana").expect("a layer for Ana");
    let ben = layer_named(&editor, "Ben").expect("a layer for Ben");
    assert!(!mouth_frames(&editor, ana).is_empty(), "Ana has no mouth keys");
    assert!(!mouth_frames(&editor, ben).is_empty(), "Ben has no mouth keys");
}

/// **Nobody mouths anybody else's lines.**
///
/// The property the whole feature exists for. Ana's lines run 0.5–2.0s and
/// 4.5–6.0s; Ben's run 2.5–4.0s and 6.5–8.0s. At 24fps those are frames 12–48,
/// 108–144 and 60–96, 156–192.
#[test]
fn a_character_is_only_animated_over_their_own_lines() {
    let (mut editor, _dir) = conversation();
    editor.lip_sync_from_captions().expect("should sync");

    let ana = layer_named(&editor, "Ana").expect("Ana");
    let ben = layer_named(&editor, "Ben").expect("Ben");

    // Every mouth keyframe must fall inside one of that speaker's own lines
    // (the closing rest lands on the frame just past the end, so the ranges are
    // inclusive of it).
    let inside = |frame: u32, spans: &[(u32, u32)]| {
        spans.iter().any(|(a, b)| frame >= *a && frame <= *b + 1)
    };
    let ana_lines = [(12u32, 48u32), (108, 144)];
    let ben_lines = [(60u32, 96u32), (156, 192)];

    for frame in mouth_frames(&editor, ana) {
        assert!(
            inside(frame, &ana_lines),
            "Ana has a mouth keyframe at {frame}, which is not one of her lines"
        );
    }
    for frame in mouth_frames(&editor, ben) {
        assert!(
            inside(frame, &ben_lines),
            "Ben has a mouth keyframe at {frame}, which is not one of his lines"
        );
    }
}

/// **The mouth closes at the end of a line.**
///
/// Without it the last shape holds until that character speaks again, leaving
/// them frozen mid-vowel through everybody else's dialogue.
#[test]
fn a_mouth_closes_when_its_line_ends() {
    let (mut editor, _dir) = conversation();
    editor.lip_sync_from_captions().expect("should sync");
    let ana = layer_named(&editor, "Ana").expect("Ana");

    let scene = editor.doc.scene();
    let l = scene.layers().get(ana).expect("Ana's layer");
    // Ana's first line ends at frame 48. Whatever is showing while Ben speaks
    // must be the rest shape, which is frame 0 of the mouth symbol.
    let showing = l
        .frames
        .resolved_at(70)
        .iter()
        .find_map(|o| match &o.kind {
            ObjectKind::Instance(i) => Some(i.first_frame),
            _ => None,
        })
        .expect("Ana should still have a mouth on screen while Ben talks");
    assert_eq!(
        showing,
        buzz_audio::Viseme::Rest.frame(),
        "Ana is left mid-vowel through Ben's line"
    );
}

/// **A speaker with no mouth is named**, because the fix — rename the symbol —
/// is one the message can state outright.
#[test]
fn a_speaker_with_no_mouth_symbol_is_named() {
    let dir = tempfile::tempdir().expect("temp dir");
    let wav = dir.path().join("vo.wav");
    std::fs::write(&wav, dialogue_wav(9.0)).expect("write wav");
    let srt = dir.path().join("vo.srt");
    std::fs::write(&srt, SRT).expect("write srt");

    let mut editor = Editor::default();
    editor.import_sound(&wav).expect("imports");
    // Only Ana has a mouth.
    editor.doc.edit("Mouths", |scene| {
        buzz_app::lipsync::placeholder_mouth(scene, "Ana");
    });
    editor.import_captions(&srt).expect("imports");

    editor.lip_sync_from_captions().expect("Ana can still sync");
    let status = editor.status.clone().unwrap_or_default();
    assert!(status.contains("Ben"), "Ben was skipped in silence: {status:?}");
}

/// **Captions that name nobody explain themselves** rather than doing nothing.
#[test]
fn captions_with_no_speakers_explain_themselves() {
    let dir = tempfile::tempdir().expect("temp dir");
    let wav = dir.path().join("vo.wav");
    std::fs::write(&wav, dialogue_wav(5.0)).expect("write wav");
    let srt = dir.path().join("plain.srt");
    std::fs::write(
        &srt,
        "1\n00:00:01,000 --> 00:00:02,000\nThe door closed.\n\n\
         2\n00:00:03,000 --> 00:00:04,000\nNobody moved.\n",
    )
    .expect("write srt");

    let mut editor = Editor::default();
    editor.import_sound(&wav).expect("imports");
    editor.import_captions(&srt).expect("imports");

    let err = editor
        .lip_sync_from_captions()
        .expect_err("narration names no speaker");
    assert!(
        err.to_string().contains("speaker"),
        "the reason should name what is missing; it said {err}"
    );
}

/// **The command is reachable from the menu and reports rather than panics.**
#[test]
fn the_command_runs_from_the_menu() {
    let (mut editor, _dir) = conversation();
    editor.run(Command::LipSyncFromCaptions);
    assert!(
        layer_named(&editor, "Ana").is_some(),
        "the command did nothing: {:?}",
        editor.status
    );
}
