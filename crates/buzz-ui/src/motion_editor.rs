//! The Motion Editor — shaping a tween's timing as a curve.
//!
//! A tween carries one easing ([`buzz_scene::Easing`]); the renderer already
//! runs it through `Easing::apply` on every frame. What was missing was a way to
//! *draw* it. This panel plots the easing of the tween under the playhead as a
//! cubic-bezier curve with two draggable control handles, plus the usual
//! presets, and hands back a new easing for the editor to write onto the
//! keyframe.
//!
//! It holds no state of its own: the handle positions are read from the tween
//! each frame, so what is on screen is always what is on the keyframe, and a
//! drag is a read-modify-write like every other panel here.

use buzz_scene::{EaseForm, Easing, TweenKind};
use egui::{Color32, Id, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

/// The four control values of an easing, as a cubic-bezier `(x1, y1, x2, y2)`
/// with implied endpoints `(0,0)` and `(1,1)`.
///
/// A `CubicBezier` gives its own; `Linear` a straight diagonal; a `Strength`
/// slider its nearest ease-in / ease-out bezier. Editing any of them writes a
/// `CubicBezier` back, so the seed is only the starting shape.
pub fn control_points(easing: Easing) -> [f64; 4] {
    match easing {
        Easing::CubicBezier { x1, y1, x2, y2 } => [x1, y1, x2, y2],
        Easing::Linear => [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
        Easing::Strength(a) if a > 0.0 => [0.0, 0.0, 0.58, 1.0], // ease out
        Easing::Strength(a) if a < 0.0 => [0.42, 0.0, 1.0, 1.0], // ease in
        Easing::Strength(_) => [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
        // Not a Bézier at all; the panel hides the handles for these, and a
        // straight seed is what "start editing handles" would begin from.
        Easing::Elastic { .. } | Easing::Bounce { .. } => {
            [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0]
        }
    }
}

/// Panel view state. Deliberately empty: the curve lives on the keyframe, not
/// here, so nothing needs remembering between frames.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MotionEditorState;

/// What the user did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MotionEditorResponse {
    /// A new easing to write onto the tween under the playhead.
    pub set_easing: Option<Easing>,
}

/// Presets as `(group, [(name, easing)])`, in the order the panel shows them.
/// The "Back" trio are plain Béziers (draggable afterwards); Elastic and
/// Bounce are their own curves a single cubic cannot express.
fn preset_groups() -> Vec<(&'static str, Vec<(&'static str, Easing)>)> {
    let bez = |x1, y1, x2, y2| Easing::CubicBezier { x1, y1, x2, y2 };
    vec![
        (
            "Basic",
            vec![
                ("Linear", Easing::Linear),
                ("Ease In", bez(0.42, 0.0, 1.0, 1.0)),
                ("Ease Out", bez(0.0, 0.0, 0.58, 1.0)),
                ("Ease In-Out", bez(0.42, 0.0, 0.58, 1.0)),
            ],
        ),
        (
            "Anticipation / overshoot",
            vec![
                ("Back In", Easing::BACK_IN),
                ("Back Out", Easing::BACK_OUT),
                ("Back In-Out", Easing::BACK_IN_OUT),
            ],
        ),
        (
            "Elastic",
            vec![
                ("Elastic In", Easing::elastic(EaseForm::In)),
                ("Elastic Out", Easing::elastic(EaseForm::Out)),
                ("Elastic In-Out", Easing::elastic(EaseForm::InOut)),
            ],
        ),
        (
            "Bounce",
            vec![
                ("Bounce In", Easing::bounce(EaseForm::In)),
                ("Bounce Out", Easing::bounce(EaseForm::Out)),
                ("Bounce In-Out", Easing::bounce(EaseForm::InOut)),
            ],
        ),
    ]
}

/// Draw the panel.
///
/// `easing`/`kind` describe the tween under the playhead, or `None` when there
/// is no tween there to shape.
pub fn motion_editor_panel(
    ui: &mut Ui,
    easing: Option<Easing>,
    kind: Option<TweenKind>,
    _state: &mut MotionEditorState,
) -> MotionEditorResponse {
    let mut out = MotionEditorResponse::default();

    ui.heading("Motion Editor");

    let Some(easing) = easing else {
        ui.add_space(6.0);
        ui.label(
            RichText::new("Put the playhead on a tween to shape its timing.")
                .small()
                .weak(),
        );
        return out;
    };

    ui.label(
        RichText::new(format!(
            "{} tween \u{2014} {}",
            kind.map(|k| k.label()).unwrap_or("Motion"),
            easing.label()
        ))
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    let mut cp = control_points(easing);

    // -- the graph ----------------------------------------------------------
    let size = ui.available_width().min(260.0);
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let painter = ui.painter_at(rect);

    // A value of 1 is the top of the box; time runs left to right. A little head
    // and foot room lets an overshoot curve show without being clipped flat.
    let pad = size * 0.18;
    let plot = Rect::from_min_max(
        pos2(rect.left(), rect.top() + pad),
        pos2(rect.right(), rect.bottom() - pad),
    );
    let to_screen = |t: f64, v: f64| -> Pos2 {
        pos2(
            plot.left() + t as f32 * plot.width(),
            plot.bottom() - v as f32 * plot.height(),
        )
    };
    let to_value = |p: Pos2| -> (f64, f64) {
        (
            ((p.x - plot.left()) / plot.width()) as f64,
            ((plot.bottom() - p.y) / plot.height()) as f64,
        )
    };

    let frame = Color32::from_gray(90);
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, Color32::from_gray(60)),
        egui::StrokeKind::Inside,
    );
    // The unit box (0..1 in both axes) and its diagonal, for reference.
    painter.rect_stroke(
        Rect::from_min_max(to_screen(0.0, 1.0), to_screen(1.0, 0.0)),
        0.0,
        Stroke::new(1.0, frame),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [to_screen(0.0, 0.0), to_screen(1.0, 1.0)],
        Stroke::new(1.0, Color32::from_gray(55)),
    );

    // The authoritative curve, sampled through the very function the renderer
    // uses — faint, so it reads as "what plays now".
    let samples: Vec<Pos2> = (0..=48)
        .map(|i| {
            let t = i as f64 / 48.0;
            to_screen(t, easing.apply(t))
        })
        .collect();
    painter.add(egui::Shape::line(
        samples,
        Stroke::new(1.5, Color32::from_gray(120)),
    ));

    let bezier = easing.is_bezier_like();
    let accent = Color32::from_rgb(0x6C, 0x8E, 0xBF);
    if !bezier {
        // Elastic / Bounce: no handles to drag, so the sampled curve itself
        // is the accent line.
        let samples: Vec<Pos2> = (0..=96)
            .map(|i| {
                let t = i as f64 / 96.0;
                to_screen(t, easing.apply(t))
            })
            .collect();
        painter.add(egui::Shape::line(samples, Stroke::new(2.0, accent)));
    }

    // The editable bezier, and its handle guide-lines.
    let p0 = to_screen(0.0, 0.0);
    let h1 = to_screen(cp[0], cp[1]);
    let h2 = to_screen(cp[2], cp[3]);
    let p3 = to_screen(1.0, 1.0);
    if bezier {
        painter.add(egui::Shape::CubicBezier(
            egui::epaint::CubicBezierShape::from_points_stroke(
                [p0, h1, h2, p3],
                false,
                Color32::TRANSPARENT,
                Stroke::new(2.0, accent),
            ),
        ));
        painter.line_segment([p0, h1], Stroke::new(1.0, accent.gamma_multiply(0.5)));
        painter.line_segment([p3, h2], Stroke::new(1.0, accent.gamma_multiply(0.5)));
    }

    // -- the two draggable handles ------------------------------------------
    let handles = if bezier { vec![h1, h2] } else { Vec::new() };
    for (i, centre) in handles.into_iter().enumerate() {
        let handle_rect = Rect::from_center_size(centre, vec2(16.0, 16.0));
        let resp = ui.interact(
            handle_rect,
            Id::new(("motion-cp", i)),
            Sense::click_and_drag(),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if resp.dragged()
            && let Some(pos) = ui.ctx().input(|input| input.pointer.interact_pos())
        {
            let (mut t, mut v) = to_value(pos);
            t = t.clamp(0.0, 1.0);
            v = v.clamp(-0.5, 1.5);
            cp[i * 2] = t;
            cp[i * 2 + 1] = v;
            out.set_easing = Some(Easing::CubicBezier {
                x1: cp[0],
                y1: cp[1],
                x2: cp[2],
                y2: cp[3],
            });
        }
        painter.circle_filled(centre, 5.0, accent);
        painter.circle_stroke(centre, 5.0, Stroke::new(1.0, Color32::WHITE));
    }

    // -- the one parameter of the non-Bézier curves --------------------------
    match easing {
        Easing::Elastic { form, oscillations } => {
            let mut n = oscillations;
            ui.horizontal(|ui| {
                ui.label("Oscillations");
                if ui
                    .add(egui::DragValue::new(&mut n).range(0.5..=12.0).speed(0.05))
                    .changed()
                {
                    out.set_easing = Some(Easing::Elastic {
                        form,
                        oscillations: n,
                    });
                }
            });
        }
        Easing::Bounce { form, bounces } => {
            let mut n = bounces;
            ui.horizontal(|ui| {
                ui.label("Bounces");
                if ui.add(egui::DragValue::new(&mut n).range(0..=10)).changed() {
                    out.set_easing = Some(Easing::Bounce { form, bounces: n });
                }
            });
        }
        _ => {}
    }

    // -- presets ------------------------------------------------------------
    ui.add_space(6.0);
    for (group, presets) in preset_groups() {
        ui.label(RichText::new(group).small().weak());
        ui.horizontal_wrapped(|ui| {
            for (name, preset) in presets {
                if ui.button(name).clicked() {
                    out.set_easing = Some(preset);
                }
            }
        });
    }
    if bezier {
        ui.label(
            RichText::new(format!(
                "({:.2}, {:.2}) ({:.2}, {:.2})",
                cp[0], cp[1], cp[2], cp[3]
            ))
            .small()
            .weak(),
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cubic_bezier_seeds_its_own_handles() {
        let e = Easing::CubicBezier {
            x1: 0.1,
            y1: 0.9,
            x2: 0.8,
            y2: 0.2,
        };
        assert_eq!(control_points(e), [0.1, 0.9, 0.8, 0.2]);
    }

    #[test]
    fn linear_and_strength_seed_monotone_handles() {
        // Every seed's x coordinates are ordered 0 <= x1 <= x2 <= 1, so the
        // curve is a function of time rather than doubling back.
        for e in [
            Easing::Linear,
            Easing::Strength(60.0),
            Easing::Strength(-60.0),
            Easing::Strength(0.0),
        ] {
            let [x1, _, x2, _] = control_points(e);
            assert!(
                (0.0..=x2).contains(&x1) && x2 <= 1.0,
                "{e:?} seeded non-monotone x: {x1}, {x2}"
            );
        }
    }

    #[test]
    fn every_preset_is_a_valid_easing() {
        // A preset must map onto a real curve the renderer can apply — and its
        // endpoints must hold (apply(0)=0, apply(1)=1), which is the contract of
        // an easing.
        for (_, presets) in preset_groups() {
            for (_, preset) in presets {
                assert!(preset.apply(0.0).abs() < 1e-6);
                assert!((preset.apply(1.0) - 1.0).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn the_back_elastic_and_bounce_presets_are_offered() {
        let names: Vec<&str> = preset_groups()
            .into_iter()
            .flat_map(|(_, p)| p)
            .map(|(n, _)| n)
            .collect();
        for want in [
            "Back In",
            "Back Out",
            "Back In-Out",
            "Elastic Out",
            "Bounce Out",
            "Bounce In-Out",
        ] {
            assert!(names.contains(&want), "{want} missing");
        }
    }

    #[test]
    fn only_bezier_curves_get_handles() {
        assert!(Easing::BACK_OUT.is_bezier_like());
        assert!(!Easing::elastic(EaseForm::Out).is_bezier_like());
        assert!(!Easing::bounce(EaseForm::In).is_bezier_like());
    }
}
