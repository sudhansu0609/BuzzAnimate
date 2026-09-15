//! **A page of prose becomes an animatic.**
//!
//! The director stages one shot. A story is several — the place changes, the
//! cast changes, and the film cuts between them — and the document already knew
//! how to hold several scenes and export them as one film. What was missing was
//! anything to fill them.

use buzz_app::editor::Editor;

const BRIEF: &str = "Night. Ana walks in from the left.\n\
                     Ana talks to Ben.\n\
                     \n\
                     Day.\n\
                     Ben walks off right.";

#[test]
fn a_brief_with_two_shots_makes_two_scenes() {
    let mut editor = Editor::default();
    assert_eq!(editor.doc.scene_names().len(), 1, "a document starts with one");

    let directed = editor.direct_sequence(BRIEF);
    assert_eq!(directed, 2, "both shots were directed");
    assert_eq!(editor.doc.scene_names().len(), 2, "a scene each");
}

/// The scene list should read like the brief, so an animatic can be navigated
/// by what happens in it rather than by number.
#[test]
fn each_scene_is_named_after_its_own_shot() {
    let mut editor = Editor::default();
    editor.direct_sequence(BRIEF);

    let names: Vec<String> = editor.doc.scene_names();
    assert!(
        names[0].to_lowercase().contains("ana"),
        "the first scene is named for its own action, got {names:?}"
    );
    assert!(
        names[1].to_lowercase().contains("ben"),
        "and so is the second, got {names:?}"
    );
}

/// Each shot really is staged and animated, not merely named: every scene has
/// a set, a cast and frames.
#[test]
fn every_scene_is_actually_staged_and_animated() {
    let mut editor = Editor::default();
    editor.direct_sequence(BRIEF);

    for index in 0..editor.doc.scene_names().len() {
        editor.doc.switch_scene(index);
        let scene = editor.doc.scene();
        assert!(
            scene.layers().len() > 1,
            "scene {index} should have a set and a cast, got {} layer(s)",
            scene.layers().len()
        );
        assert!(
            scene.frame_count() > 1,
            "scene {index} should have been animated, got {} frame(s)",
            scene.frame_count()
        );
    }
}

/// A brief with nothing to cut behaves exactly as it always did: one shot, in
/// the scene that was already open.
#[test]
fn a_single_shot_brief_stays_one_scene() {
    let mut editor = Editor::default();
    let directed = editor.direct_sequence("Night. Ana walks in from the left.");
    assert_eq!(directed, 1);
    assert_eq!(
        editor.doc.scene_names().len(),
        1,
        "nothing said to cut, so nothing was cut"
    );
}

/// Everything directed before a failure stays, and the message says where it
/// stopped — far more useful than discarding a page because the last paragraph
/// was a fragment.
#[test]
fn a_shot_it_cannot_read_stops_the_run_and_says_where() {
    let mut editor = Editor::default();
    let directed = editor.direct_sequence("Night. Ana walks in from the left.\n\n???");
    assert_eq!(directed, 1, "the first shot survives");

    let status = editor.status.clone().unwrap_or_default();
    assert!(
        status.contains("shot 2"),
        "it should name the shot it stopped at, got {status:?}"
    );
}

#[test]
fn an_empty_brief_directs_nothing() {
    let mut editor = Editor::default();
    assert_eq!(editor.direct_sequence("   \n\n  "), 0);
    assert_eq!(editor.doc.scene_names().len(), 1, "and adds no scenes");
}
