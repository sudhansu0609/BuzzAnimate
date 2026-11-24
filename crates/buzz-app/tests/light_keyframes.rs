//! **Animating a light from the keyframe commands.**
//!
//! The light track's maths is unit-tested in `buzz-light`; what these check is
//! the editor wiring the timeline and the Light menu drive — that keying the
//! selected light at the playhead builds its track, that removing a key clears
//! it, that keying needs a light, and that two keys actually interpolate. Driven
//! through `Editor::run` so the command path is exercised, not just a helper.

use buzz_app::editor::Editor;
use buzz_geom::Point;
use buzz_scene::{LightId, LightKind};
use buzz_ui::Command;

fn key_count(editor: &Editor, id: LightId) -> usize {
    editor
        .doc
        .scene()
        .lights()
        .get(id)
        .and_then(|l| l.track.as_ref())
        .map_or(0, |t| t.keys().len())
}

fn has_key(editor: &Editor, id: LightId, frame: u32) -> bool {
    editor
        .doc
        .scene()
        .lights()
        .get(id)
        .and_then(|l| l.track.as_ref())
        .is_some_and(|t| t.has_key_at(frame))
}

#[test]
fn keying_the_selected_light_builds_its_track() {
    let mut editor = Editor::default();
    editor.add_light(LightKind::lamp(Point::new(200.0, 200.0)));
    let id = editor.light_panel.selected.expect("add_light selects it");
    assert_eq!(key_count(&editor, id), 0, "a fresh light has no track");

    editor.current_frame = 0;
    editor.run(Command::AddLightKeyframe);
    assert_eq!(key_count(&editor, id), 1, "the first key");

    editor.current_frame = 12;
    editor.run(Command::AddLightKeyframe);
    assert_eq!(key_count(&editor, id), 2, "the second key");
    assert!(has_key(&editor, id, 12));

    // Playhead still on 12: removing keys there.
    editor.run(Command::RemoveLightKeyframe);
    assert_eq!(key_count(&editor, id), 1, "the key at the playhead is gone");
    assert!(!has_key(&editor, id, 12));
    assert!(has_key(&editor, id, 0), "the other key is untouched");
}

#[test]
fn keying_without_a_selected_light_does_nothing() {
    let mut editor = Editor::default();
    editor.add_light(LightKind::sun());
    editor.light_panel.selected = None;
    editor.run(Command::AddLightKeyframe); // must not panic, must key nothing

    let any_keyed = editor
        .doc
        .scene()
        .lights()
        .lights
        .iter()
        .any(|l| l.track.as_ref().is_some_and(|t| !t.keys().is_empty()));
    assert!(!any_keyed, "nothing should be keyed with no light selected");
}

#[test]
fn two_keys_interpolate_the_light_between_them() {
    let mut editor = Editor::default();
    editor.add_light(LightKind::lamp(Point::new(100.0, 200.0)));
    let id = editor.light_panel.selected.unwrap();

    editor.current_frame = 0;
    editor.run(Command::AddLightKeyframe);
    let start = match editor.doc.scene().lights().get(id).unwrap().kind {
        LightKind::Lamp { position, .. } => position,
        _ => panic!("expected a lamp"),
    };

    // Move the lamp at frame 20 as the stage's own drag does — onto the
    // frame's state, then keyed there — 200 to the right.
    editor.current_frame = 20;
    editor.run(Command::AddLightKeyframe);
    editor.doc.edit("Move Lamp", |scene| {
        if let Some(light) = scene.lights_mut().get_mut(id)
            && let Some(track) = light.track.as_mut()
        {
            let mut keys: Vec<_> = track.keys().to_vec();
            for key in &mut keys {
                if key.frame == 20
                    && let LightKind::Lamp { position, .. } = &mut key.kind
                {
                    position.x += 200.0;
                }
                // A light's keys hold, as Animate's do, until the span between
                // them is tweened.
                if key.frame == 0 {
                    key.tween = true;
                }
            }
            for key in keys {
                track.set_key(key);
            }
        }
    });

    // Half way between, the resolved lamp sits between the two keyed positions.
    let rig = editor.doc.scene().lights().resolved_at(10);
    let light = rig.get(id).expect("the light");
    match light.kind {
        LightKind::Lamp { position, .. } => assert!(
            position.x > start.x + 20.0 && position.x < start.x + 180.0,
            "midpoint x was {}, not between {} and {}",
            position.x,
            start.x,
            start.x + 200.0
        ),
        _ => panic!("expected a lamp"),
    }
    // And untweened, it holds the first key's position until the second.
    let rig = editor.doc.scene().lights().resolved_at(19);
    assert!(rig.get(id).is_some());
}

#[test]
fn keying_is_one_undo_step() {
    let mut editor = Editor::default();
    editor.add_light(LightKind::lamp(Point::new(200.0, 200.0)));
    let id = editor.light_panel.selected.unwrap();
    editor.current_frame = 5;
    editor.run(Command::AddLightKeyframe);
    // A first key after frame 1 also keys frame 1 as the light stands, so the
    // change made at this key does not reach back to the start.
    assert_eq!(key_count(&editor, id), 2);
    assert!(has_key(&editor, id, 0) && has_key(&editor, id, 5));

    editor.doc.undo();
    assert_eq!(key_count(&editor, id), 0, "one Ctrl+Z takes both back");
}
