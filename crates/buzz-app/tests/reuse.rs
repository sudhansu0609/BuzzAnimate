//! **One performance, many characters. One swap, every instance.**
//!
//! The multiplier a solo animator actually needs: a walk authored once driving
//! a whole cast, and a costume change that does not mean replacing every
//! instance by hand and typing its position back in.

use buzz_app::editor::Editor;
use buzz_geom::{Affine, Rect, Shape as _};
use buzz_scene::{LayerId, ObjectId, ObjectKind, ShapeData, SymbolKind};
use peniko::Color;

// ---------------------------------------------------------------------------
// Swapping a symbol
// ---------------------------------------------------------------------------

/// A document with two symbols and three instances of the first.
fn two_symbols() -> (Editor, buzz_scene::SymbolId, buzz_scene::SymbolId, Vec<ObjectId>) {
    let mut editor = Editor::default();
    let layer = editor.doc.scene().layers().iter().next().expect("a layer").id;

    let mut coat = buzz_scene::SymbolId(0);
    let mut jacket = buzz_scene::SymbolId(0);
    let mut placed = Vec::new();

    editor.doc.edit("setup", |scene| {
        let art = |c: Color| {
            ShapeData::filled(Rect::new(0.0, 0.0, 40.0, 80.0).to_path(1e-9), c)
        };
        coat = scene.add_symbol("Coat", SymbolKind::Graphic, None);
        scene.library_mut().update(coat, |symbol| {
            let inner = symbol.layers.iter().next().map(|l| l.id);
            if let Some(inner) = inner {
                symbol.layers.update(inner, |l| {
                    l.frames.push_object(
                        0,
                        std::sync::Arc::new(buzz_scene::Object::shape(
                            ObjectId(9001),
                            art(Color::from_rgb8(0xC0, 0x30, 0x20)),
                        )),
                    );
                });
            }
        });
        jacket = scene.add_symbol("Jacket", SymbolKind::Graphic, None);

        for i in 0..3 {
            if let Some(id) = scene.add_instance_at(
                layer,
                0,
                coat,
                Affine::translate((100.0 * f64::from(i), 0.0)),
            ) {
                placed.push(id);
            }
        }
    });
    (editor, coat, jacket, placed)
}

fn symbol_of(editor: &Editor, id: ObjectId) -> Option<buzz_scene::SymbolId> {
    match &editor.doc.scene().find_object(id)?.1.kind {
        ObjectKind::Instance(instance) => Some(instance.symbol),
        _ => None,
    }
}

#[test]
fn swapping_a_symbol_repoints_every_instance() {
    let (mut editor, coat, jacket, placed) = two_symbols();
    assert_eq!(placed.len(), 3, "three instances were placed");

    let mut swapped = 0;
    editor.doc.edit("swap", |scene| {
        swapped = scene.swap_symbol(coat, jacket);
    });
    assert_eq!(swapped, 3);

    for id in &placed {
        assert_eq!(symbol_of(&editor, *id), Some(jacket));
    }
}

/// **The placement survives.** An instance carries where it stands, how big it
/// is and what colour effect it has; swapping the drawing must not cost any of
/// that, or it would be no better than deleting and replacing by hand.
#[test]
fn a_swap_keeps_where_each_instance_stands() {
    let (mut editor, coat, jacket, placed) = two_symbols();
    let before: Vec<Affine> = placed
        .iter()
        .map(|id| {
            editor
                .doc
                .scene()
                .find_object(*id)
                .expect("the instance")
                .1
                .transform
        })
        .collect();

    editor.doc.edit("swap", |scene| {
        scene.swap_symbol(coat, jacket);
    });

    for (id, was) in placed.iter().zip(before) {
        let now = editor
            .doc
            .scene()
            .find_object(*id)
            .expect("the instance")
            .1
            .transform;
        assert_eq!(
            now.as_coeffs(),
            was.as_coeffs(),
            "the instance stayed where it was put"
        );
    }
}

#[test]
fn swapping_a_symbol_for_itself_does_nothing() {
    let (mut editor, coat, _jacket, _placed) = two_symbols();
    let mut swapped = 1;
    editor.doc.edit("swap", |scene| {
        swapped = scene.swap_symbol(coat, coat);
    });
    assert_eq!(swapped, 0);
}

// ---------------------------------------------------------------------------
// Retargeting a performance
// ---------------------------------------------------------------------------

/// Two identical rigs on their own layers, the first with a performance on it.
fn two_rigs() -> (Editor, ObjectId, ObjectId) {
    let mut editor = Editor::default();
    let first = editor.doc.scene().layers().iter().next().expect("a layer").id;
    let mut second = LayerId(0);
    editor.doc.edit("layers", |scene| {
        second = scene.add_layer("Second", buzz_scene::LayerKind::Normal);
    });

    let a = stand_a_figure(&mut editor, first);
    let b = stand_a_figure(&mut editor, second);

    // A performance on the first: a walk, written as pose keyframes.
    let performance = buzz_act::perform::Performance::new(buzz_act::perform::Action::Walk, 0..24);
    editor.doc.edit("perform", |scene| {
        buzz_act::perform::apply(scene, a, &performance).expect("it performs");
    });
    (editor, a, b)
}

fn stand_a_figure(editor: &mut Editor, layer: LayerId) -> ObjectId {
    let mut id = None;
    editor.doc.edit("figure", |scene| {
        let spec = buzz_act::figure::FigureSpec::default();
        let object_id = scene.next_object_id();
        let figure = buzz_act::figure::build(&spec, object_id, || scene.next_object_id());
        id = scene.add_object(layer, figure);
    });
    id.expect("a figure")
}

fn pose_at(editor: &Editor, id: ObjectId, frame: u32) -> Option<Vec<f64>> {
    let scene = editor.doc.scene();
    let (layer, _) = scene.find_object(id)?;
    scene
        .layers()
        .get(layer)?
        .frames
        .resolved_at(frame)
        .iter()
        .find(|o| o.id == id)
        .and_then(|o| match &o.kind {
            ObjectKind::Armature(rig) => Some(rig.armature.pose()),
            _ => None,
        })
}

#[test]
fn one_walk_drives_a_second_character() {
    let (mut editor, a, b) = two_rigs();
    let source_pose = pose_at(&editor, a, 6).expect("the first rig is posed");

    editor.selection.set([a, b]);
    editor.retarget_performance();

    let copied = pose_at(&editor, b, 6).expect("the second rig is posed now");
    assert_eq!(
        copied, source_pose,
        "the second character stands as the first does on that frame"
    );
}

/// It refuses rather than folding a character through itself.
#[test]
fn rigs_with_different_skeletons_are_refused() {
    let (mut editor, a, _b) = two_rigs();
    // A plain shape is not a rig at all.
    let layer = editor.doc.scene().layers().iter().next().expect("a layer").id;
    let mut plain = None;
    editor.doc.edit("shape", |scene| {
        plain = scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::BLACK),
        );
    });
    let plain = plain.expect("a shape");

    editor.selection.set([a, plain]);
    editor.retarget_performance();
    assert!(
        editor
            .status
            .as_deref()
            .is_some_and(|s| s.contains("not a rig")),
        "it should say what is wrong, got {:?}",
        editor.status
    );
}

#[test]
fn retargeting_needs_exactly_two() {
    let (mut editor, a, _b) = two_rigs();
    editor.selection.set([a]);
    editor.retarget_performance();
    assert!(
        editor
            .status
            .as_deref()
            .is_some_and(|s| s.contains("Select two rigs")),
        "got {:?}",
        editor.status
    );
}
