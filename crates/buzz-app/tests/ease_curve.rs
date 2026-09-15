//! **Shaping a tween's timing from the Motion Editor.**
//!
//! The curve maths lives in `buzz-scene`; the panel is egui. This checks the
//! join the editor owns: setting an ease curve replaces only the easing on the
//! keyframe under the playhead, keeps the tween's kind, changes what the tween
//! actually does, and is one undo step.

use buzz_app::editor::Editor;
use buzz_geom::{Rect, Shape as _};
use buzz_scene::{Easing, LayerId, ShapeData, Tween, TweenKind};
use peniko::Color;

fn editor_with_motion_tween() -> (Editor, LayerId) {
    let mut editor = Editor::default();
    let layer = editor.doc.scene().layers().iter().next().expect("a layer").id;
    editor.doc.edit("setup", |scene| {
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
        );
        scene.update_layer(layer, |l| {
            if l.frames.length() <= 10 {
                l.frames.insert_frame(10);
            }
        });
        scene.ensure_keyframe(layer, 10);
        scene.update_layer(layer, |l| {
            l.frames.set_tween(0, Tween::motion());
        });
    });
    editor.selection.set_active_layer(Some(layer));
    editor.current_frame = 3;
    (editor, layer)
}

fn tween_at(editor: &Editor, layer: LayerId, frame: u32) -> Tween {
    editor
        .doc
        .scene()
        .layers()
        .get(layer)
        .unwrap()
        .frames
        .tween_at(frame)
}

#[test]
fn setting_a_curve_replaces_only_the_easing() {
    let (mut editor, layer) = editor_with_motion_tween();
    assert_eq!(tween_at(&editor, layer, 3).easing, Easing::Linear);

    // A strong ease-in.
    let curve = Easing::CubicBezier {
        x1: 0.9,
        y1: 0.0,
        x2: 1.0,
        y2: 1.0,
    };
    editor.set_ease_curve(curve);

    let tween = tween_at(&editor, layer, 3);
    assert_eq!(tween.easing, curve, "the curve was written");
    assert_eq!(tween.kind, TweenKind::Motion, "the tween kind is preserved");
    // A strong ease-in sits well below the linear midpoint at half time.
    assert!(
        tween.easing.apply(0.5) < 0.35,
        "the curve should actually ease: apply(0.5) = {}",
        tween.easing.apply(0.5)
    );
}

#[test]
fn setting_a_curve_is_one_undo_step() {
    let (mut editor, layer) = editor_with_motion_tween();
    editor.set_ease_curve(Easing::CubicBezier {
        x1: 0.9,
        y1: 0.0,
        x2: 1.0,
        y2: 1.0,
    });
    assert!(matches!(tween_at(&editor, layer, 3).easing, Easing::CubicBezier { .. }));

    editor.doc.undo();
    assert_eq!(
        tween_at(&editor, layer, 3).easing,
        Easing::Linear,
        "one Ctrl+Z restores the previous easing"
    );
}

#[test]
fn a_preset_curve_can_be_set_too() {
    let (mut editor, layer) = editor_with_motion_tween();
    editor.set_ease_curve(Easing::Linear);
    assert_eq!(tween_at(&editor, layer, 3).easing, Easing::Linear);
}
