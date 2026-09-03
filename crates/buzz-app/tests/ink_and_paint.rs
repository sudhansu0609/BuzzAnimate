//! **Ink and paint: colouring a drawing once, not once per frame.**
//!
//! Colouring is half the labour of drawn animation and almost none of the
//! craft. The line art is redrawn every frame; the colours are the same on every
//! frame. Traditional pipelines have automated this for decades under that name.
//!
//! What is tested here is the behaviour that makes it safe to run: it colours
//! what it can, it never overwrites what a person did, running it twice does
//! nothing the second time, and where it cannot place a region it says so
//! instead of guessing.

use buzz_app::editor::Editor;
use buzz_geom::{Point, Rect, Shape as _};
use buzz_scene::{LayerId, ObjectKind, ShapeData};
use peniko::Color;

const INK: Color = Color::from_rgb8(0x10, 0x10, 0x10);
const COAT: Color = Color::from_rgb8(0xC0, 0x30, 0x20);

/// A closed outline — the "line art" of one frame — drawn as a stroked box at
/// `x`, so the enclosure moves frame to frame the way a redrawn one does.
fn outline(editor: &mut Editor, layer: LayerId, frame: u32, x: f64) {
    editor.doc.edit("draw", |scene| {
        if frame > 0 {
            scene.update_layer(layer, |l| {
                while l.frames.length() <= frame {
                    l.frames.insert_frame(l.frames.length());
                }
                l.frames.insert_blank_keyframe(frame);
            });
        }
        scene.add_shape_at(
            layer,
            frame,
            ShapeData::stroked(
                Rect::new(x, 40.0, x + 120.0, 160.0).to_path(1e-9),
                INK,
                2.0,
            ),
        );
    });
}

/// How many bucket fills are on this frame.
fn fills(editor: &Editor, layer: LayerId, frame: u32) -> usize {
    editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .expect("the layer")
        .frames
        .resolved_at(frame)
        .iter()
        .filter(|o| match &o.kind {
            ObjectKind::Shape(s) => s
                .fill
                .as_ref()
                .is_some_and(|f| f.rule == buzz_scene::bucket::FILL_RULE),
            _ => false,
        })
        .count()
}

fn fill_colour(editor: &Editor, layer: LayerId, frame: u32) -> Option<Color> {
    editor
        .doc
        .scene()
        .layers()
        .get(layer)?
        .frames
        .resolved_at(frame)
        .iter()
        .find_map(|o| match &o.kind {
            ObjectKind::Shape(s) => s
                .fill
                .as_ref()
                .filter(|f| f.rule == buzz_scene::bucket::FILL_RULE)
                .map(|f| f.paint.color()),
            _ => None,
        })
}

/// Three frames of a shape that drifts a little, coloured once on the first.
fn a_short_sequence(drift: f64) -> (Editor, LayerId) {
    let mut editor = Editor::default();
    let layer = editor.doc.scene().layers().iter().next().expect("a layer").id;
    for frame in 0..3u32 {
        outline(&mut editor, layer, frame, 100.0 + drift * f64::from(frame));
    }
    // Colour frame 1 by hand, with the bucket, exactly as a person would.
    editor.current_frame = 0;
    editor.style.fill_color = COAT;
    editor.style.fill_enabled = true;
    editor.apply(buzz_app::tools::ToolAction::BucketFill {
        point: Point::new(160.0, 100.0),
    });
    (editor, layer)
}

#[test]
fn colouring_one_frame_colours_the_ones_after_it() {
    let (mut editor, layer) = a_short_sequence(6.0);
    assert_eq!(fills(&editor, layer, 0), 1, "frame 1 was coloured by hand");
    assert_eq!(fills(&editor, layer, 1), 0, "and the rest are bare");

    let (painted, missed) = editor.propagate_fills(2);
    assert_eq!(painted, 2, "both later frames were coloured");
    assert_eq!(missed, 0);

    for frame in 0..3u32 {
        assert_eq!(fills(&editor, layer, frame), 1, "frame {frame}");
        assert_eq!(
            fill_colour(&editor, layer, frame)
                .expect("a colour")
                .to_rgba8()
                .to_u8_array(),
            COAT.to_rgba8().to_u8_array(),
            "frame {frame} is the same colour as the one it came from"
        );
    }
}

/// Running it twice does nothing the second time, so a repeated menu click
/// cannot stack fills on top of each other.
#[test]
fn running_it_again_changes_nothing() {
    let (mut editor, layer) = a_short_sequence(6.0);
    editor.propagate_fills(2);
    let before: Vec<usize> = (0..3).map(|f| fills(&editor, layer, f)).collect();

    let (painted, _) = editor.propagate_fills(2);
    assert_eq!(painted, 0, "there was nothing left to colour");
    let after: Vec<usize> = (0..3).map(|f| fills(&editor, layer, f)).collect();
    assert_eq!(before, after);
}

/// **It never paints over a person's own work.** A frame somebody coloured
/// differently on purpose keeps that colour.
#[test]
fn a_frame_coloured_by_hand_is_left_alone() {
    let (mut editor, layer) = a_short_sequence(6.0);

    // Colour the middle frame a different colour, deliberately.
    let other = Color::from_rgb8(0x20, 0x80, 0xC0);
    editor.current_frame = 1;
    editor.style.fill_color = other;
    editor.apply(buzz_app::tools::ToolAction::BucketFill {
        point: Point::new(166.0, 100.0),
    });
    assert_eq!(fills(&editor, layer, 1), 1);

    editor.current_frame = 0;
    editor.propagate_fills(2);

    assert_eq!(
        fill_colour(&editor, layer, 1)
            .expect("a colour")
            .to_rgba8()
            .to_u8_array(),
        other.to_rgba8().to_u8_array(),
        "the hand-coloured frame kept its own colour"
    );
    assert_eq!(fills(&editor, layer, 1), 1, "and gained no second fill");
}

/// **A region that has moved too far is left, and counted.** A wrong colour
/// looks deliberate and survives to the film; a missing one is visible at once
/// and is a click to fix.
#[test]
fn a_drawing_that_moved_too_far_is_reported_not_guessed() {
    // Drifting further than the shape is wide takes the seed outside it.
    let (mut editor, layer) = a_short_sequence(400.0);
    let (painted, missed) = editor.propagate_fills(2);

    assert_eq!(painted, 0, "nothing could be placed");
    assert_eq!(missed, 2, "and it says how many");
    assert_eq!(fills(&editor, layer, 1), 0, "no colour was guessed at");
    assert!(
        editor
            .status
            .as_deref()
            .is_some_and(|s| s.contains("could not be placed") || s.contains("Could not place")),
        "it should say so, got {:?}",
        editor.status
    );
}

/// Line art is a filled shape too — a brush stroke is — and carrying those
/// forward would draw the drawing twice. Only the bucket's own fills travel.
#[test]
fn line_art_is_not_mistaken_for_paint() {
    let mut editor = Editor::default();
    let layer = editor.doc.scene().layers().iter().next().expect("a layer").id;
    for frame in 0..2u32 {
        editor.doc.edit("draw", |scene| {
            if frame > 0 {
                scene.update_layer(layer, |l| {
                    while l.frames.length() <= frame {
                        l.frames.insert_frame(l.frames.length());
                    }
                    l.frames.insert_blank_keyframe(frame);
                });
            }
            // A *filled* shape, as a brush stroke is — but not a bucket fill.
            scene.add_shape_at(
                layer,
                frame,
                ShapeData::filled(Rect::new(100.0, 40.0, 220.0, 160.0).to_path(1e-9), INK),
            );
        });
    }
    editor.current_frame = 0;

    let (painted, missed) = editor.propagate_fills(1);
    assert_eq!((painted, missed), (0, 0), "there is no paint here to carry");
    assert!(
        editor
            .status
            .as_deref()
            .is_some_and(|s| s.contains("no bucket fills")),
        "it should say there was nothing to carry, got {:?}",
        editor.status
    );
}
