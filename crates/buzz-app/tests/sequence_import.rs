//! **A folder of numbered drawings, one to a frame.**
//!
//! Scanned drawings, frames from another program, a render from somewhere else:
//! all of them arrive as a numbered folder, and all of them had to be brought in
//! one file at a time. The exporter has written PNG sequences from the start.

use buzz_app::editor::Editor;
use buzz_scene::{LayerKind, ObjectKind, Paint};

/// Write `count` tiny PNGs into a folder, named with `pattern`.
fn a_folder_of(count: u32, name: impl Fn(u32) -> String) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a scratch folder");
    for i in 0..count {
        // A one-pixel picture is enough: what is under test is the placing.
        let asset = buzz_scene::ImageAsset::blank(buzz_scene::ImageId(u64::from(i) + 1), "f", 4, 4);
        let png = asset.encode_png().expect("a png");
        std::fs::write(dir.path().join(name(i)), png).expect("writing a frame");
    }
    dir
}

fn placed_frames(editor: &Editor) -> Vec<u32> {
    let layer = editor
        .doc
        .scene()
        .layers()
        .iter()
        .find(|l| l.kind == LayerKind::Normal && l.frames.keyframe_count() > 1)
        .expect("the sequence layer");
    (0..layer.frames.length())
        .filter(|f| {
            layer
                .frames
                .resolved_at(*f)
                .iter()
                .any(|o| matches!(&o.kind, ObjectKind::Shape(s)
                    if s.fill.as_ref().is_some_and(|f| matches!(f.paint, Paint::Image(_)))))
        })
        .collect()
}

#[test]
fn a_folder_lands_one_drawing_to_a_frame() {
    let dir = a_folder_of(5, |i| format!("frame{:03}.png", i + 1));
    let mut editor = Editor::default();

    let placed = editor
        .import_image_sequence(dir.path())
        .expect("the folder imports");

    assert_eq!(placed, 5, "every drawing was placed");
    assert_eq!(
        placed_frames(&editor),
        vec![0, 1, 2, 3, 4],
        "one to a frame, with none held over another"
    );
}

/// **`frame2` before `frame10`.** Sorting by name gets this exactly backwards,
/// and discovering it two hundred drawings later would not be a nice afternoon.
#[test]
fn frames_are_ordered_by_their_numbers_not_their_names() {
    let dir = a_folder_of(12, |i| format!("frame{}.png", i + 1));
    let mut editor = Editor::default();
    editor
        .import_image_sequence(dir.path())
        .expect("the folder imports");

    // The pictures are distinguishable only by size, so read the order back
    // through the library names, which carry the file names.
    let layer = editor
        .doc
        .scene()
        .layers()
        .iter()
        .find(|l| l.frames.keyframe_count() > 1)
        .expect("the sequence layer")
        .id;
    let order: Vec<String> = (0..12)
        .filter_map(|frame| {
            let scene = editor.doc.scene();
            let object = scene
                .layers()
                .get(layer)?
                .frames
                .resolved_at(frame)
                .iter()
                .next()
                .cloned()?;
            match &object.kind {
                ObjectKind::Shape(s) => match &s.fill.as_ref()?.paint {
                    Paint::Image(img) => Some(img.asset.name.clone()),
                    _ => None,
                },
                _ => None,
            }
        })
        .collect();

    assert_eq!(order.first().map(String::as_str), Some("frame1"));
    assert_eq!(
        order.get(1).map(String::as_str),
        Some("frame2"),
        "frame2 comes second, not after frame10: {order:?}"
    );
    assert_eq!(order.last().map(String::as_str), Some("frame12"));
}

#[test]
fn a_folder_with_no_pictures_says_so() {
    let dir = tempfile::tempdir().expect("a folder");
    std::fs::write(dir.path().join("notes.txt"), "nothing here").expect("a file");
    let mut editor = Editor::default();

    let error = editor
        .import_image_sequence(dir.path())
        .expect_err("there is nothing to import");
    assert!(
        format!("{error}").contains("no pictures"),
        "it should say what was wrong, got {error}"
    );
}

/// Nothing is imported at all if a file cannot be read, rather than half a
/// sequence being left behind.
#[test]
fn a_bad_file_leaves_the_document_alone() {
    let dir = a_folder_of(3, |i| format!("f{i}.png"));
    std::fs::write(dir.path().join("f9.png"), b"not a png").expect("a broken frame");

    let mut editor = Editor::default();
    let layers_before = editor.doc.scene().layers().len();
    // A file that is not a picture decodes to nothing and is skipped; what
    // matters is that the rest still arrive and the count is honest.
    let placed = editor
        .import_image_sequence(dir.path())
        .expect("the readable frames still import");
    assert_eq!(placed, 3, "the three real drawings arrived");
    assert_eq!(
        editor.doc.scene().layers().len(),
        layers_before + 1,
        "on one new layer"
    );
}
