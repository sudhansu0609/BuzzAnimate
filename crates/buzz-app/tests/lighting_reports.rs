//! **Three reports, and what was actually wrong.**
//!
//! > *"I still cannot see a different colour of light. If I use the light once
//! > and cancel it, next time the light does not show up. I also could not see
//! > the darkness again."*
//!
//! The renderer was not the problem — the exporter's and the stage's lighting
//! tests all passed, and a lamp's colour moves the picture by a hundred levels
//! when you measure it. What was wrong was that a light could be *created*
//! unable to be seen, and that nothing guaranteed a lighting change a frame of
//! its own once it had been.
//!
//! 1. **Every lamp after the first landed on exactly the same point.** The
//!    position was a fixed fraction of the view, so a second lamp arrived on
//!    top of the first: the picture barely moved, which is indistinguishable
//!    from the light not having been added. Delete one and add another and the
//!    new one appears where the old one was, which is the same report worded
//!    the other way round.
//!
//! 2. **A light added while the stage had no area was born too small to see.**
//!    The reach and the throw were sized from the visible rectangle alone, and
//!    that rectangle is a *point* before the stage is laid out, while the
//!    Lighting panel is maximised over it, or while the window is minimised. A
//!    lamp got the minimum reach of forty units on a stage five hundred and
//!    fifty across; a gloom got a throw of one unit and a width of one and a
//!    half. Both exist, both are in the panel, and neither can be found on the
//!    stage. Zooming in did a milder version of the same thing.
//!
//! 3. **The window's retained stage encoding keyed the lighting on the document
//!    revision**, and undo puts the revision back — so one number can describe
//!    two different rigs.
//!
//! The pixel-level proofs live beside these, in `stage_lighting.rs`.

use buzz_app::editor::Editor;
use buzz_doc::Document;
use buzz_geom::{Point, Rect, Shape as _, Size};
use buzz_scene::{LayerKind, LightKind, Scene, ShapeData};
use peniko::Color;

/// The stage the user's own test document has: 550 × 400, artwork across it.
fn editor() -> Editor {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(
            Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9),
            Color::from_rgb8(0xE8, 0xE4, 0xDC),
        ),
    );
    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(900.0, 600.0);
    editor.camera.center = Point::new(275.0, 200.0);
    editor.camera.zoom = 1.0;
    editor
}

fn lamp_of(editor: &Editor, index: usize) -> (Point, f64) {
    match editor.scene().lights().lights[index].kind {
        LightKind::Lamp {
            position, radius, ..
        } => (position, radius),
        other => panic!("not a lamp: {other:?}"),
    }
}

fn gloom_of(editor: &Editor, index: usize) -> (f64, f64) {
    match editor.scene().lights().lights[index].kind {
        LightKind::Gloom { throw, width, .. } => (throw, width),
        other => panic!("not a gloom: {other:?}"),
    }
}

/// **Report 1.** Two lamps in the same place look like one lamp.
#[test]
fn a_new_lamp_never_lands_on_one_already_there() {
    let mut editor = editor();
    editor.add_light(LightKind::lamp(Point::ORIGIN));
    editor.add_light(LightKind::lamp(Point::ORIGIN));
    editor.add_light(LightKind::lamp(Point::ORIGIN));

    let places: Vec<Point> = (0..3).map(|i| lamp_of(&editor, i).0).collect();
    for (i, a) in places.iter().enumerate() {
        for b in &places[i + 1..] {
            assert!(
                (*a - *b).hypot() > 20.0,
                "two lamps arrived on top of each other, so the second reads as \
                 doing nothing: {a:?} and {b:?}"
            );
        }
        assert!(
            editor.scene().stage().stage_rect().inflate(200.0, 200.0).contains(*a),
            "a new lamp must still arrive somewhere near the picture: {a:?}"
        );
    }
}

/// The same thing said the way the report said it: delete the light, add
/// another, and the new one must not simply reoccupy the old one's place.
#[test]
fn a_lamp_added_after_one_was_deleted_is_a_lamp_you_can_find() {
    let mut editor = editor();
    editor.add_light(LightKind::lamp(Point::ORIGIN));
    let (first, reach) = lamp_of(&editor, 0);

    editor.doc.edit("Delete Light", |scene| {
        let id = scene.lights().lights[0].id;
        scene.lights_mut().remove(id);
    });
    assert!(editor.scene().lights().lights.is_empty());

    editor.add_light(LightKind::lamp(Point::ORIGIN));
    let (second, again) = lamp_of(&editor, 0);

    // With the rig empty it is right for the new lamp to take the good spot
    // again — what must not happen is that it arrives unable to light anything.
    assert_eq!(second, first, "the only lamp belongs in the key position");
    assert_eq!(again, reach);
    assert!(
        again >= 150.0,
        "a lamp on a 550-wide stage needs a reach that crosses it: {again}"
    );
}

/// **Report 2.** The stage has no area until it has been laid out, and a light
/// added in that moment used to be born too small to see.
#[test]
fn a_light_added_before_the_stage_is_laid_out_is_still_visible() {
    let mut editor = editor();
    // What the camera holds on the first frames, and any time the stage is
    // given no room: an empty viewport, so the visible rectangle is a point.
    editor.camera.viewport = Size::new(0.0, 0.0);
    assert_eq!(editor.camera.visible_doc_rect().area(), 0.0);

    editor.add_light(LightKind::lamp(Point::ORIGIN));
    let (_, reach) = lamp_of(&editor, 0);
    assert!(
        reach >= 150.0,
        "a lamp born with no view got the minimum reach and could not be seen \
         on a 550-wide stage: {reach}"
    );

    editor.add_light(LightKind::gloom(Point::ORIGIN));
    let (throw, width) = gloom_of(&editor, 1);
    let stage = editor.scene().stage().stage_rect();
    let span = stage.width().hypot(stage.height());
    assert!(
        throw >= span * 0.9 && width >= span * 0.9,
        "a gloom born with no view was a speck rather than a wall: \
         throw {throw}, width {width}, against a stage span of {span}"
    );
}

/// Zoomed into a detail, a new light must still light the *shot*. Sized to the
/// magnified rectangle it lit the detail and died before the edge of the frame,
/// so zooming back out showed a picture that was barely lit at all.
#[test]
fn a_light_added_while_zoomed_in_still_lights_the_shot() {
    let mut editor = editor();
    editor.camera.zoom = 8.0;
    let seen = editor.camera.visible_doc_rect();
    assert!(seen.width() < 200.0, "the test needs a magnified view");

    editor.add_light(LightKind::lamp(Point::ORIGIN));
    let (_, reach) = lamp_of(&editor, 0);
    assert!(
        reach >= 150.0,
        "a lamp sized to a magnified detail cannot light the shot: {reach}"
    );

    editor.add_light(LightKind::gloom(Point::ORIGIN));
    let (throw, _) = gloom_of(&editor, 1);
    let stage = editor.scene().stage().stage_rect();
    assert!(
        throw >= stage.width(),
        "a gloom sized to a magnified detail is a grey wedge in the middle of \
         the picture rather than a wall of dark: {throw}"
    );
}

/// **Report 3, and why the stage encoding may not key the lighting on the
/// document revision.**
///
/// The revision is a clock on the document's content, and undo puts it *back*.
/// So one revision number can describe two different lighting rigs, and the
/// retained stage encoding — which is reused whenever its stamp matches — would
/// have no way to tell them apart. The stamp carries
/// [`buzz_scene::LightRig::fingerprint`] for exactly this reason; this pins the
/// premise, which is the half that can silently stop being true.
#[test]
fn undo_puts_the_revision_back_so_it_cannot_key_the_lighting() {
    let mut editor = editor();
    let start = editor.scene().revision();

    editor.add_light(LightKind::lamp(Point::new(100.0, 100.0)));
    let after_lamp = editor.scene().revision();
    let lamp_rig = editor.scene().lights().fingerprint();
    assert!(after_lamp > start);

    assert!(editor.doc.undo(), "the add must be undoable");
    assert_eq!(
        editor.scene().revision(),
        start,
        "undo restores the revision rather than moving it on"
    );

    // A *different* light, from the same starting point.
    editor.add_light(LightKind::gloom(Point::ORIGIN));
    assert_eq!(
        editor.scene().revision(),
        after_lamp,
        "the revision has come back round to a number it has already been"
    );
    assert_ne!(
        editor.scene().lights().fingerprint(),
        lamp_rig,
        "two different rigs at the same revision: the revision cannot be what \
         tells the window its retained stage is out of date"
    );
}

/// A gloom's own numbers must survive being added, whatever the view. The
/// panel's position is deliberately discarded — a wall the width of the picture
/// has no business being dropped where the pointer was — so this pins that what
/// replaces it is a wall rather than nothing.
#[test]
fn a_gloom_is_always_born_wide_enough_to_be_a_wall() {
    for (what, zoom, viewport) in [
        ("fitted", 1.0, Size::new(900.0, 600.0)),
        ("magnified", 8.0, Size::new(900.0, 600.0)),
        ("no stage area", 1.0, Size::new(0.0, 0.0)),
        ("a sliver of stage", 1.0, Size::new(4.0, 3.0)),
    ] {
        let mut editor = editor();
        editor.camera.zoom = zoom;
        editor.camera.viewport = viewport;
        editor.add_light(LightKind::lamp(Point::ORIGIN));
        editor.add_light(LightKind::gloom(Point::ORIGIN));

        let (throw, width) = gloom_of(&editor, 1);
        let stage = editor.scene().stage().stage_rect();
        let span = stage.width().hypot(stage.height());
        assert!(
            throw >= span * 0.9,
            "{what}: the throw must cross the picture, not stop inside it: \
             {throw} against {span}"
        );
        assert!(
            width >= span * 0.9,
            "{what}: the wall must be wider than the picture: {width} against {span}"
        );
    }
}

/// **Insert ▸ Light ▸ Fire, in one command.**
///
/// The report was "where is the fire?", and it was a fair question: the preset
/// was a button inside the selected light's panel section, which you can only
/// reach by adding a lamp, finding the panel, selecting the lamp and scrolling
/// to it. A preset that has to be hunted for is a preset nobody uses.
#[test]
fn adding_a_fire_gives_a_lamp_that_gutters() {
    let mut editor = editor();
    editor.run(buzz_ui::Command::AddFire);

    let light = editor
        .scene()
        .lights()
        .lights
        .last()
        .expect("a fire was added");
    assert!(matches!(light.kind, LightKind::Lamp { .. }), "a fire is a lamp");
    assert!(light.flicker > 0.0, "and it gutters");
    assert!(
        editor.scene().lights().animates(),
        "so the rig animates with no keyframes in it"
    );

    // And it arrives sized to the shot, like every other light: a fire born
    // with the minimum reach is a fire nobody can see.
    let LightKind::Lamp { radius, .. } = light.kind else {
        unreachable!()
    };
    assert!(radius >= 100.0, "a fire on a 550-wide stage needs reach: {radius}");
}

/// A second fire must not land on the first, and must not gutter in step with
/// it — two fires in one shot are two fires.
#[test]
fn two_fires_are_two_fires() {
    let mut editor = editor();
    editor.run(buzz_ui::Command::AddFire);
    editor.run(buzz_ui::Command::AddFire);

    let lights = &editor.scene().lights().lights;
    assert_eq!(lights.len(), 2);
    let places: Vec<Point> = lights
        .iter()
        .map(|l| match l.kind {
            LightKind::Lamp { position, .. } => position,
            other => panic!("not a lamp: {other:?}"),
        })
        .collect();
    assert!(
        (places[0] - places[1]).hypot() > 20.0,
        "the second fire arrived on the first: {places:?}"
    );

    let in_step = (0..40)
        .filter(|f| lights[0].flickered(*f).intensity == lights[1].flickered(*f).intensity)
        .count();
    assert!(in_step < 3, "{in_step} of forty frames guttered together");
}
