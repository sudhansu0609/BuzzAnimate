//! The Layer Depth panel.
//!
//! Animate's Layer Depth panel does two things at once, and both matter: it
//! lets you set how far each layer sits from the camera, and it *shows* you
//! the arrangement, because a column of numbers is a poor way to understand a
//! spatial relationship.
//!
//! # The view is side-on, not through the camera
//!
//! The stage already shows what the camera sees. Repeating that here would add
//! nothing — the useful question is "how are my layers arranged in space?",
//! which the camera's own view cannot answer, since a layer twice as far and
//! twice as big looks identical through the lens. So this draws the scene from
//! the side: the camera on the left, depth running to the right, and each
//! layer as a plane whose height falls off with distance exactly as the
//! renderer's perspective does. Where two layers sit at the same depth, they
//! land on the same line — which is the thing you came here to find out.

use buzz_scene::{LayerId, Scene};
use egui::{Color32, RichText, Ui};

use crate::theme::Palette;

/// What the user did in the panel.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DepthResponse {
    /// A layer's depth was changed.
    pub set_depth: Option<(LayerId, f64)>,
    /// The camera's focal distance was changed.
    pub set_focal_distance: Option<f64>,
    /// A layer row was clicked.
    pub select_layer: Option<LayerId>,
    /// Put every layer back on the focal plane.
    pub flatten: bool,
    /// Space the layers out evenly, front to back.
    pub distribute: bool,
}

/// The depth range the panel offers.
///
/// Bounded well short of the camera: a layer may be brought forward, but the
/// slider will not take it to the plane where the projection blows up. Typing
/// a value past this is still possible through the document, and the renderer
/// handles it by not drawing the layer — but a slider should not lead you
/// somewhere the picture disappears.
const NEAR_LIMIT: f64 = -0.9;
const FAR_LIMIT: f64 = 4.0;

/// Draw the Layer Depth panel.
pub fn depth_panel(ui: &mut Ui, scene: &Scene, active: Option<LayerId>) -> DepthResponse {
    let mut response = DepthResponse::default();
    let focal = scene.camera().focal_distance;

    ui.horizontal(|ui| {
        ui.heading("Layer Depth");
        ui.label(
            RichText::new(format!("{} layers", scene.layers().len()))
                .small()
                .weak(),
        );
    });

    // -- the side-on view ---------------------------------------------------
    perspective_view(ui, scene, active, &mut response);

    ui.add_space(4.0);

    // -- camera depth -------------------------------------------------------
    ui.horizontal(|ui| {
        ui.label("Camera depth");
        let mut value = focal;
        // **The slider takes what is left of the row, not egui's fixed 100
        // points.** A label, a 100-point slider and its number box come to
        // more than a dock column at its narrowest, and the number box — the
        // half you can type into — was the part that ended up off the panel.
        ui.spacing_mut().slider_width = (ui.available_width() - 66.0).max(40.0);
        if ui
            .add(
                egui::Slider::new(&mut value, 200.0..=6000.0)
                    .logarithmic(true)
                    .suffix(" px"),
            )
            .on_hover_text(
                "How far the camera sits from the stage. Closer exaggerates \
                 perspective; further flattens it.",
            )
            .changed()
        {
            response.set_focal_distance = Some(value);
        }
    });

    ui.horizontal(|ui| {
        if ui
            .small_button("Flatten")
            .on_hover_text("Put every layer back on the focal plane")
            .clicked()
        {
            response.flatten = true;
        }
        if ui
            .small_button("Distribute")
            .on_hover_text("Space the layers evenly, front layer nearest")
            .clicked()
        {
            response.distribute = true;
        }
    });

    ui.separator();

    // -- per-layer depth ----------------------------------------------------
    // The dock column already scrolls; see the note on `tool_bar`.

    let rows: Vec<(LayerId, String, f64, Color32)> = scene
        .layers()
        .iter()
        .map(|l| {
            let [r, g, b, a] = l.color.to_rgba8().to_u8_array();
            (
                l.id,
                l.name.clone(),
                l.depth,
                Color32::from_rgba_unmultiplied(r, g, b, a),
            )
        })
        .collect();

    for (id, name, depth, colour) in rows {
        ui.horizontal(|ui| {
            let (chip, _) = ui.allocate_exact_size(egui::vec2(7.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(chip, 1.0, colour);

            // Truncated: a layer name has no length limit, and the depth field
            // on the right of this row is what a long one pushes off the panel.
            if ui
                .add(
                    egui::Button::selectable(active == Some(id), &name)
                        .truncate()
                        .min_size(egui::vec2((ui.available_width() - 64.0).max(1.0), 0.0)),
                )
                .clicked()
            {
                response.select_layer = Some(id);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut value = depth;
                if ui
                    .add(
                        egui::DragValue::new(&mut value)
                            .speed(5.0)
                            .range(focal * NEAR_LIMIT..=focal * FAR_LIMIT),
                    )
                    .changed()
                {
                    response.set_depth = Some((id, value));
                }
            });
        });
    }

    response
}

/// The scene from the side: camera at the left, depth increasing rightwards.
fn perspective_view(ui: &mut Ui, scene: &Scene, active: Option<LayerId>, out: &mut DepthResponse) {
    let (rect, view) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 132.0),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, Palette::panel());

    let focal = scene.camera().focal_distance.max(1.0);

    // The horizontal axis runs from the camera to the furthest layer, with the
    // focal plane always shown so the reference is never off-screen.
    let deepest = scene
        .layers()
        .iter()
        .map(|l| l.depth)
        .fold(0.0_f64, f64::max)
        .max(focal * 0.6);
    let span = focal + deepest;

    let left = rect.left() + 26.0;
    let right = rect.right() - 10.0;
    let axis_y = rect.center().y;
    // Distance from the camera, in the panel's own pixels.
    let to_x = |distance: f64| -> f32 {
        let t = (distance / span).clamp(0.0, 1.0);
        left + (right - left) * t as f32
    };

    // The camera, as an eye at the origin of the axis.
    painter.circle_filled(egui::pos2(left, axis_y), 4.0, Palette::text());
    painter.text(
        egui::pos2(rect.left() + 4.0, axis_y - 14.0),
        egui::Align2::LEFT_CENTER,
        "cam",
        egui::FontId::proportional(9.0),
        Palette::text_dim(),
    );

    // The focal plane, dashed, because it is the reference every depth is
    // measured against.
    let focal_x = to_x(focal);
    for i in 0..9 {
        let y0 = rect.top() + 10.0 + i as f32 * 13.0;
        painter.line_segment(
            [egui::pos2(focal_x, y0), egui::pos2(focal_x, y0 + 6.0)],
            egui::Stroke::new(1.0, Palette::border()),
        );
    }
    painter.text(
        egui::pos2(focal_x, rect.bottom() - 6.0),
        egui::Align2::CENTER_CENTER,
        "stage",
        egui::FontId::proportional(9.0),
        Palette::text_dim(),
    );

    // Each layer as a plane, drawn at the height perspective gives it. Front
    // of the stack last, so it is drawn over the others.
    let layers: Vec<_> = scene.layers().iter().cloned().collect();
    for layer in layers.iter().rev() {
        let distance = focal + layer.depth;
        // Behind the camera: nothing to draw, but say so rather than leave a
        // gap the user cannot explain.
        if distance <= 1.0 {
            painter.text(
                egui::pos2(left, rect.bottom() - 6.0),
                egui::Align2::LEFT_CENTER,
                format!("{} is behind the camera", layer.name),
                egui::FontId::proportional(9.0),
                Color32::from_rgb(0xE0, 0x70, 0x50),
            );
            continue;
        }

        let x = to_x(distance);
        // The same falloff the renderer uses, so the picture does not lie.
        let scale = (focal / distance) as f32;
        let half = 44.0 * scale;

        let [r, g, b, _] = layer.color.to_rgba8().to_u8_array();
        let selected = active == Some(layer.id);
        let colour = Color32::from_rgba_unmultiplied(r, g, b, if selected { 255 } else { 170 });

        painter.line_segment(
            [egui::pos2(x, axis_y - half), egui::pos2(x, axis_y + half)],
            egui::Stroke::new(if selected { 3.0 } else { 2.0 }, colour),
        );
        // Sight lines from the camera to the plane's edges, which is what makes
        // the drawing read as a perspective frustum rather than a bar chart.
        if selected {
            for edge in [axis_y - half, axis_y + half] {
                painter.line_segment(
                    [egui::pos2(left, axis_y), egui::pos2(x, edge)],
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(r, g, b, 60)),
                );
            }
        }
    }

    // Clicking a plane selects that layer, which is the quickest way to reach
    // the layer you can see is in the wrong place.
    if view.clicked()
        && let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos())
    {
        let nearest = layers
            .iter()
            .filter(|l| focal + l.depth > 1.0)
            .min_by(|a, b| {
                let da = (to_x(focal + a.depth) - pos.x).abs();
                let db = (to_x(focal + b.depth) - pos.x).abs();
                da.total_cmp(&db)
            })
            .map(|l| l.id);
        out.select_layer = nearest;
    }
}

/// Depths that space `count` layers evenly from the focal plane backwards.
///
/// The front layer stays where the stage is and the rest recede, so
/// distributing never pulls artwork towards the camera unexpectedly — the
/// picture keeps its framing and simply gains depth.
pub fn distributed_depths(count: usize, focal_distance: f64) -> Vec<f64> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![0.0];
    }
    // Spread over one focal distance, which is a full halving of scale from
    // front to back — visible without being absurd.
    let step = focal_distance / (count - 1) as f64;
    (0..count).map(|i| i as f64 * step).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::LayerKind;

    fn scene_with_layers(n: usize) -> Scene {
        let mut scene = Scene::default();
        for i in 1..n {
            scene.add_layer(format!("Layer {i}"), LayerKind::Normal);
        }
        scene
    }

    #[test]
    fn distributing_puts_the_front_layer_on_the_stage_and_the_rest_behind() {
        let depths = distributed_depths(5, 1000.0);
        assert_eq!(depths.len(), 5);
        assert_eq!(depths[0], 0.0, "the front layer stays on the focal plane");

        // Strictly increasing, so no two layers share a depth.
        for pair in depths.windows(2) {
            assert!(pair[1] > pair[0], "{depths:?}");
        }
        assert_eq!(*depths.last().unwrap(), 1000.0);
    }

    /// Nothing is ever pulled towards the camera, so distributing cannot make
    /// a layer vanish behind it.
    #[test]
    fn distributing_never_moves_a_layer_in_front_of_the_stage() {
        for count in [1, 2, 7, 40] {
            for depth in distributed_depths(count, 800.0) {
                assert!(depth >= 0.0, "count {count} produced {depth}");
            }
        }
    }

    #[test]
    fn distributing_nothing_or_one_layer_is_well_defined() {
        assert!(distributed_depths(0, 1000.0).is_empty());
        assert_eq!(distributed_depths(1, 1000.0), vec![0.0]);
    }

    #[test]
    fn the_panel_draws_without_panicking() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let scene = scene_with_layers(4);
        let active = scene.layers().iter().next().map(|l| l.id);

        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = depth_panel(ui, &scene, active);
        });
    }

    /// A layer the renderer refuses to draw must not crash the panel that is
    /// meant to help you find it.
    #[test]
    fn the_panel_survives_a_layer_behind_the_camera() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);

        let mut scene = scene_with_layers(3);
        let id = scene.layers().iter().next().unwrap().id;
        scene.update_layer(id, |l| l.depth = -5000.0);

        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = depth_panel(ui, &scene, Some(id));
        });
    }

    #[test]
    fn an_empty_document_draws_a_usable_panel() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let scene = Scene::empty();

        let _ = ctx.run_ui(Default::default(), |ui| {
            let _ = depth_panel(ui, &scene, None);
        });
    }
}
