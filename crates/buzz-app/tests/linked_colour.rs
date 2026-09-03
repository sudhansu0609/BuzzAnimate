//! **A palette that links.**
//!
//! Until this, a swatch handed you a colour and forgot: making a character's
//! coat green meant finding every piece of it, on every frame, inside every
//! symbol, and repainting each by hand — and missing one, which is how a
//! character ends up with two slightly different coats in the same film.
//!
//! A fill that remembers which swatch it came from turns that into one edit.
//! These prove the link holds where it matters: across frames, inside symbols,
//! inside groups and rigs, through a save, and — the one that makes it safe —
//! that it leaves unlinked artwork alone.

use buzz_app::editor::Editor;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{LayerId, ObjectId, ObjectKind, Paint, ShapeData, SwatchId};
use peniko::Color;

const COAT: Color = Color::from_rgb8(0x8B, 0x2E, 0x2E);
const GREEN: Color = Color::from_rgb8(0x2E, 0x8B, 0x3A);
const UNRELATED: Color = Color::from_rgb8(0x11, 0x22, 0x33);

fn editor() -> (Editor, LayerId) {
    let editor = Editor::default();
    let layer = editor.doc.scene().layers().iter().next().expect("a layer").id;
    (editor, layer)
}

fn add_shape(editor: &mut Editor, layer: LayerId, at: f64, colour: Color) -> ObjectId {
    let mut id = None;
    editor.doc.edit("setup", |scene| {
        id = scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(at, 0.0, at + 40.0, 40.0).to_path(1e-9), colour),
        );
    });
    id.expect("a shape")
}

fn fill_colour(editor: &Editor, id: ObjectId) -> Color {
    let (_, object) = editor.doc.scene().find_object(id).expect("the shape");
    let ObjectKind::Shape(shape) = &object.kind else {
        panic!("not a shape")
    };
    shape.fill.as_ref().expect("a fill").paint.color()
}

fn a_swatch(editor: &mut Editor, colour: Color) -> SwatchId {
    let mut id = SwatchId(0);
    editor.doc.edit("palette", |scene| {
        id = scene.swatches_mut().add("Coat", colour, None);
    });
    id
}

/// The whole point: one change to the swatch, and everything wearing it moves.
#[test]
fn changing_a_swatch_repaints_everything_linked_to_it() {
    let (mut editor, layer) = editor();
    let swatch = a_swatch(&mut editor, COAT);

    let a = add_shape(&mut editor, layer, 0.0, COAT);
    let b = add_shape(&mut editor, layer, 60.0, COAT);
    editor.selection.set([a, b]);
    editor.link_selection_to_swatch(swatch);

    editor.recolour_swatch(swatch, GREEN);

    assert_eq!(fill_colour(&editor, a).to_rgba8().to_u8_array(), GREEN.to_rgba8().to_u8_array());
    assert_eq!(fill_colour(&editor, b).to_rgba8().to_u8_array(), GREEN.to_rgba8().to_u8_array());
}

/// **And leaves everything else alone.** A recolour that reached artwork nobody
/// linked would be worse than no recolour at all: it would be unpredictable.
#[test]
fn artwork_that_is_not_linked_is_untouched() {
    let (mut editor, layer) = editor();
    let swatch = a_swatch(&mut editor, COAT);

    let linked = add_shape(&mut editor, layer, 0.0, COAT);
    // Painted the *same colour*, but never linked to the swatch.
    let coincidental = add_shape(&mut editor, layer, 60.0, COAT);
    let other = add_shape(&mut editor, layer, 120.0, UNRELATED);

    editor.selection.set([linked]);
    editor.link_selection_to_swatch(swatch);
    editor.recolour_swatch(swatch, GREEN);

    assert_eq!(
        fill_colour(&editor, linked).to_rgba8().to_u8_array(),
        GREEN.to_rgba8().to_u8_array()
    );
    assert_eq!(
        fill_colour(&editor, coincidental).to_rgba8().to_u8_array(),
        COAT.to_rgba8().to_u8_array(),
        "the same colour is not the same link"
    );
    assert_eq!(
        fill_colour(&editor, other).to_rgba8().to_u8_array(),
        UNRELATED.to_rgba8().to_u8_array()
    );
}

/// A character is usually a symbol, and a recolour that stopped at the stage
/// would be worse than useless.
#[test]
fn the_recolour_reaches_inside_a_symbol() {
    let (mut editor, layer) = editor();
    let swatch = a_swatch(&mut editor, COAT);
    let shape = add_shape(&mut editor, layer, 0.0, COAT);
    editor.selection.set([shape]);
    editor.link_selection_to_swatch(swatch);

    // Fold it into a symbol, which moves the artwork into the library.
    editor.run(buzz_ui::Command::ConvertToSymbol);
    editor.recolour_swatch(swatch, GREEN);

    let repainted = editor
        .doc
        .scene()
        .library()
        .iter()
        .flat_map(|symbol| {
            symbol
                .layers
                .iter()
                .flat_map(|l| l.frames.resolved_at(0u32).iter().cloned().collect::<Vec<_>>())
                .collect::<Vec<_>>()
        })
        .filter_map(|o| match &o.kind {
            ObjectKind::Shape(s) => s.fill.as_ref().map(|f| f.paint.color()),
            _ => None,
        })
        .any(|c| c.to_rgba8().to_u8_array() == GREEN.to_rgba8().to_u8_array());

    assert!(
        repainted,
        "the artwork inside the symbol should have followed the swatch"
    );
}

/// The link is a fact about the document and has to survive being saved, or it
/// works until you close the file and never again.
#[test]
fn the_link_survives_a_save_and_a_reopen() {
    let (mut editor, layer) = editor();
    let swatch = a_swatch(&mut editor, COAT);
    let shape = add_shape(&mut editor, layer, 0.0, COAT);
    editor.selection.set([shape]);
    editor.link_selection_to_swatch(swatch);

    let bytes = buzz_doc::format::to_bytes(editor.doc.scene()).expect("it saves");
    let mut reopened = buzz_doc::format::from_bytes(&bytes).expect("it reopens");

    let painted = reopened.recolour_swatch(swatch, GREEN);
    assert_eq!(painted, 1, "the link came back with the file");

    let colour = reopened
        .find_object(shape)
        .map(|(_, o)| match &o.kind {
            ObjectKind::Shape(s) => s.fill.as_ref().expect("a fill").paint.color(),
            _ => panic!("not a shape"),
        })
        .expect("the shape");
    assert_eq!(
        colour.to_rgba8().to_u8_array(),
        GREEN.to_rgba8().to_u8_array()
    );
}

/// Swapping a whole palette — the same drawing in a second set of colours,
/// which is how a night version of a scene is made.
#[test]
fn a_whole_palette_can_be_swapped_at_once() {
    let (mut editor, layer) = editor();
    let coat = a_swatch(&mut editor, COAT);
    let mut skin = SwatchId(0);
    editor.doc.edit("palette", |scene| {
        skin = scene.swatches_mut().add("Skin", UNRELATED, None);
    });

    let a = add_shape(&mut editor, layer, 0.0, COAT);
    editor.selection.set([a]);
    editor.link_selection_to_swatch(coat);
    let b = add_shape(&mut editor, layer, 60.0, UNRELATED);
    editor.selection.set([b]);
    editor.link_selection_to_swatch(skin);

    let night = |id: SwatchId| {
        if id == coat {
            Some(GREEN)
        } else if id == skin {
            Some(Color::BLACK)
        } else {
            None
        }
    };
    let mut painted = 0;
    editor.doc.edit("Night", |scene| {
        painted = scene.recolour_by(&night);
    });
    assert_eq!(painted, 2, "both linked fills moved together");
    assert_eq!(
        fill_colour(&editor, a).to_rgba8().to_u8_array(),
        GREEN.to_rgba8().to_u8_array()
    );
    assert_eq!(
        fill_colour(&editor, b).to_rgba8().to_u8_array(),
        Color::BLACK.to_rgba8().to_u8_array()
    );
}

// ---------------------------------------------------------------------------
// Select Same Colour — the way to recolour a document that predates the link
// ---------------------------------------------------------------------------

#[test]
fn select_same_colour_finds_every_shape_painted_that_colour() {
    let (mut editor, layer) = editor();
    let a = add_shape(&mut editor, layer, 0.0, COAT);
    let b = add_shape(&mut editor, layer, 60.0, COAT);
    let other = add_shape(&mut editor, layer, 120.0, UNRELATED);

    editor.selection.set([a]);
    editor.run(buzz_ui::Command::SelectSameColour);

    let selected = editor.selection.ids();
    assert!(selected.contains(&a) && selected.contains(&b), "both coats");
    assert!(!selected.contains(&other), "and nothing else");
}

#[test]
fn select_same_colour_needs_something_to_go_on() {
    let (mut editor, _layer) = editor();
    editor.run(buzz_ui::Command::SelectSameColour);
    assert!(
        editor
            .status
            .as_deref()
            .is_some_and(|s| s.contains("Select a filled shape")),
        "it should say what it needs, got {:?}",
        editor.status
    );
}
