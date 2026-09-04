//! **Captions in and out, through the editor.**
//!
//! The format itself is unit-tested in `buzz-doc`. What that cannot show is the
//! thing the feature is for: read a subtitle file and the words are *on the
//! timeline*, on the frames they are spoken, as real text objects — and writing
//! them back out gives a file that says the same thing.
//!
//! The round trip is the assertion that matters. A caption pipeline that
//! quietly shifts everything by a frame, or loses the last line, is worse than
//! none: you would not find out until it was on the picture.

use buzz_app::editor::Editor;
use buzz_scene::LayerId;
use buzz_ui::Command;

const SRT: &str = "\
1
00:00:01,000 --> 00:00:03,000
We should go before it gets dark.

2
00:00:04,000 --> 00:00:06,500
Ben said nothing at all.

3
00:00:08,000 --> 00:00:10,000
The door closed behind them.
";

fn editor_with_captions() -> (Editor, tempfile::TempDir, usize) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("vo.srt");
    std::fs::write(&path, SRT).expect("write srt");

    let mut editor = Editor::default();
    let n = editor
        .import_captions(&path)
        .expect("the subtitles should import");
    (editor, dir, n)
}

fn caption_layer(editor: &Editor) -> LayerId {
    editor
        .doc
        .scene()
        .layers()
        .iter()
        .find(|l| l.name == "Captions")
        .map(|l| l.id)
        .expect("a Captions layer")
}

/// Every text string on a layer at a frame.
fn text_at(editor: &Editor, layer: LayerId, frame: u32) -> Vec<String> {
    editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .map(|l| {
            l.frames
                .resolved_at(frame)
                .iter()
                .filter_map(|o| o.text.as_ref().map(|t| t.content.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// **The words land on the frames they are spoken on.**
#[test]
fn captions_arrive_on_their_own_timecodes() {
    let (editor, _dir, n) = editor_with_captions();
    assert_eq!(n, 3);
    let layer = caption_layer(&editor);

    // At 24fps: 1s is frame 24, 4s is 96, 8s is 192.
    assert_eq!(
        text_at(&editor, layer, 24),
        vec!["We should go before it gets dark.".to_string()],
        "the first line is not on its own frame"
    );
    assert_eq!(
        text_at(&editor, layer, 96),
        vec!["Ben said nothing at all.".to_string()]
    );
    assert_eq!(
        text_at(&editor, layer, 192),
        vec!["The door closed behind them.".to_string()]
    );
}

/// **A caption goes away when the line ends.**
///
/// Without a blank keyframe at the end of a cue the last line of a scene hangs
/// on the screen to the end of the film — which is the kind of thing nobody
/// notices until it is rendered.
#[test]
fn a_caption_leaves_the_screen_when_the_line_ends() {
    let (editor, _dir, _) = editor_with_captions();
    let layer = caption_layer(&editor);

    // The first cue ends at 3s — frame 72. By 3.5s (84) there should be nothing.
    assert!(
        text_at(&editor, layer, 84).is_empty(),
        "the first caption is still up between the lines: {:?}",
        text_at(&editor, layer, 84)
    );
    // And the gap before the third line is clear too.
    assert!(text_at(&editor, layer, 180).is_empty());
}

/// **Captions go on a layer of their own**, so re-importing after a re-cut
/// throws away a layer rather than picking text out of a drawing — and it is
/// the layer left selected, so exporting straight back out needs no thought.
#[test]
fn captions_land_on_their_own_layer_ready_to_export() {
    let (mut editor, dir, _) = editor_with_captions();
    let layer = caption_layer(&editor);
    assert!(
        editor.doc.scene().layers().get(layer).is_some(),
        "no Captions layer was made"
    );
    // Export takes the *active* layer; if the import did not leave its own
    // layer selected this writes nothing.
    let out = dir.path().join("straight-back.srt");
    assert_eq!(
        editor.export_captions(&out).expect("should export"),
        3,
        "the caption layer was not the one left selected"
    );
}

/// **In and out and in again says the same thing.** The property that makes it
/// safe to round-trip captions through the program at all.
#[test]
fn captions_survive_a_round_trip() {
    let (mut editor, dir, _) = editor_with_captions();
    let out = dir.path().join("out.srt");
    let written = editor.export_captions(&out).expect("captions should write");
    assert_eq!(written, 3, "{:?}", editor.status);

    let there = buzz_doc::srt::parse(SRT);
    let back = buzz_doc::srt::parse(&std::fs::read_to_string(&out).expect("read back"));
    assert_eq!(back.cues.len(), there.cues.len());
    for (a, b) in there.cues.iter().zip(&back.cues) {
        assert_eq!(a.text, b.text, "the words changed");
        // Frame-quantised on the way through, so a rounding of up to half a
        // frame each way is expected and anything more is a bug.
        let slack = (1000.0_f64 / 24.0 / 2.0).ceil() as u64 + 1;
        assert!(
            a.start_ms.abs_diff(b.start_ms) <= slack,
            "the line moved: {} ms became {} ms",
            a.start_ms,
            b.start_ms
        );
    }
}

/// **A file that is not subtitles says so** rather than making an empty layer.
#[test]
fn a_file_that_is_not_subtitles_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "Just some notes I wrote about the film.\n").expect("write");

    let mut editor = Editor::default();
    let result = editor.import_captions(&path);
    assert!(result.is_err(), "a text file was accepted as subtitles");
    assert!(
        editor
            .doc
            .scene()
            .layers()
            .iter()
            .all(|l| l.name != "Captions"),
        "a layer was made for a file that could not be read"
    );
}

/// **Exporting a layer with no text explains itself.** Silence here would look
/// like the command not working.
#[test]
fn exporting_a_drawing_layer_explains_itself() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut editor = Editor::default();
    let err = editor
        .export_captions(&dir.path().join("out.srt"))
        .expect_err("a drawing layer has no captions");
    assert!(
        err.to_string().contains("no text"),
        "the reason should name what is missing; it said {err}"
    );
}

/// **The command is reachable and does not panic without a path.** The dialogs
/// belong to the shell; the editor must simply not act on its own.
#[test]
fn the_menu_commands_are_inert_without_a_file() {
    let mut editor = Editor::default();
    editor.run(Command::ImportCaptions);
    editor.run(Command::ExportCaptions);
    assert!(
        editor.doc.scene().layers().iter().all(|l| l.name != "Captions"),
        "the command acted without being given a file"
    );
}
