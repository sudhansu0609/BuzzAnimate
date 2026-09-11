//! **A cast you can re-skin.**
//!
//! The Cast panel keeps characters where they outlive the film, and lets you
//! change what they wear from the panel — change the skin or the coat once and
//! it changes on every part of the character, and stays changed. These prove
//! the round trip the panel performs: a rigged character is saved into the
//! cast, re-skinned by matching a colour, written back, and comes back the new
//! colour on every part — and still rigged, so it can still be posed and
//! directed.

use buzz_act::{FigureSpec, build_figure, is_figure};
use buzz_doc::AssetLibrary;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{LayerKind, Scene, ShapeData};
use peniko::Color;

/// A rigged character with a bit of drawn costume on top, so the wardrobe has
/// both a rig's parts and a plain shape to reach.
fn a_character() -> Scene {
    let mut scene = Scene::default();
    let layer = scene.add_layer("Ana", LayerKind::Normal);

    let id = scene.next_object_id();
    let figure = build_figure(&FigureSpec::default(), id, || scene.next_object_id());
    scene.add_object(layer, figure);

    scene
}

/// **The whole promise, through the same doors the app uses.** A character is
/// saved into the cast, the most-worn colour is changed once, written back, and
/// on reload every part that wore it wears the new colour instead — and nothing
/// else does.
#[test]
fn re_skinning_a_character_changes_every_part_and_sticks() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut library = AssetLibrary::at(dir.path());

    // Filed under the cast folder, which is what tells a character from a prop.
    let asset = library
        .save("Ana", buzz_ui::CAST_FOLDER, &a_character())
        .expect("save the character");
    assert_eq!(asset.folder, "Cast");

    // The wardrobe: every colour the character wears, most-worn first.
    let mut scene = library.load(&asset).expect("read the character");
    let palette = scene.colours_used();
    assert!(!palette.is_empty(), "a drawn character wears something");
    let (worn, count) = palette[0];

    // Change that colour, exactly as the panel's well does.
    let new = Color::from_rgb8(0x01, 0x99, 0xEE);
    let painted = scene.recolour_matching(worn, new);
    assert_eq!(painted, count, "every part wearing it was repainted, and only those");

    // Write it back, and read it fresh off disk — the change is the
    // character's from now on.
    library.save("Ana", buzz_ui::CAST_FOLDER, &scene).expect("save back");
    let reloaded = library.load(&asset).expect("read it again");

    let after = reloaded.colours_used();
    assert!(
        after.iter().any(|(c, n)| *c == new && *n == count),
        "the new colour is worn by every part that wore the old one: {after:?}"
    );
    assert!(
        after.iter().all(|(c, _)| *c != worn),
        "no part still wears the old colour: {after:?}"
    );

    // And it is still a rigged character, so it can still be posed and cast.
    let still_rigged = reloaded
        .layers()
        .iter()
        .flat_map(|l| l.objects_at(0).iter())
        .any(|o| is_figure(o));
    assert!(still_rigged, "re-skinning must not cost the character its rig");
}

/// Re-skinning to a colour nothing wears, or to the colour already worn,
/// changes nothing — the well should not report a phantom change.
#[test]
fn re_skinning_to_an_absent_or_identical_colour_is_a_no_op() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut library = AssetLibrary::at(dir.path());
    let asset = library
        .save("Ana", buzz_ui::CAST_FOLDER, &a_character())
        .expect("save");

    let mut scene = library.load(&asset).expect("read");
    let worn = scene.colours_used()[0].0;

    assert_eq!(
        scene.recolour_matching(Color::from_rgb8(1, 2, 3), Color::BLACK),
        0,
        "a colour the character does not wear reaches nothing"
    );
    assert_eq!(
        scene.recolour_matching(worn, worn),
        0,
        "changing a colour to itself is not a change"
    );
}

/// A plain drawn part re-skins too — the wardrobe is not only the rig.
#[test]
fn a_drawn_costume_part_re_skins() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut library = AssetLibrary::at(dir.path());

    let mut scene = Scene::default();
    let layer = scene.add_layer("Cap", LayerKind::Normal);
    let cap = Color::from_rgb8(0x7A, 0x1F, 0x1F);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(0.0, 0.0, 20.0, 20.0).to_path(1e-9), cap),
    );
    let asset = library.save("Ben", buzz_ui::CAST_FOLDER, &scene).expect("save");

    let mut loaded = library.load(&asset).expect("read");
    let new = Color::from_rgb8(0x20, 0x60, 0xC0);
    assert_eq!(loaded.recolour_matching(cap, new), 1, "the cap was repainted");
    library.save("Ben", buzz_ui::CAST_FOLDER, &loaded).expect("save back");

    let reloaded = library.load(&asset).expect("read again");
    assert!(
        reloaded.colours_used().iter().any(|(c, _)| *c == new),
        "the new cap colour survived the save"
    );
}
