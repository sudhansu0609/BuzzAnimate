//! Drawing the stage: artwork through Vello, chrome through egui.
//!
//! # Why the split
//!
//! Artwork goes through Vello because it is real vector geometry that must
//! survive unbounded zoom. Chrome — rulers, guides, grid, selection handles —
//! is fixed-size interface drawn in *screen* space, so it belongs to egui's
//! painter, which already sits on top of the rendered frame.
//!
//! Keeping them apart also means chrome never lands in an exported frame,
//! which is exactly the distinction Animate draws between the stage and what
//! gets published.

use buzz_geom::{Affine, Point, Rect, Vec2};
use buzz_render::SceneBuilder;
use buzz_scene::{LayerKind, Object, ObjectKind};
use buzz_ui::{Metrics, Orientation, Palette};
use egui::{Align2, Color32, FontId, Sense, Stroke, StrokeKind, Ui};
use peniko::Color;

use crate::editor::Editor;
use crate::tools::Preview;

/// What the user did to the stage chrome this frame.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ChromeResponse {
    /// A guide was dragged off a ruler.
    pub new_guide: Option<buzz_ui::Guide>,
    /// The pointer is over the stage, so tools should receive events.
    pub hovered: bool,
}

/// Encode the document into a Vello scene.
///
/// `area` is the region of the window the stage occupies; the camera's viewport
/// carries its size and this supplies its origin.
pub fn build_scene(vello: &mut vello::Scene, editor: &Editor, area: Rect) {
    let mut builder = SceneBuilder::new(vello, &editor.camera)
        .with_viewport_offset(Vec2::new(area.x0, area.y0));

    let scene = editor.scene();

    // The stage rectangle. Everything outside it is pasteboard, which the
    // window's clear colour already provides.
    builder.fill_shape(&scene.stage().stage_rect(), scene.stage().background);

    let frame = editor.current_frame;
    // The camera is part of the document, so it transforms artwork but not the
    // stage rectangle the artwork sits on.
    let camera = scene.camera_transform(frame);

    // Onion skin ghosts first, so the live frame draws over them.
    for ghost_frame in editor.onion_frames() {
        let distance = ghost_frame.abs_diff(frame).max(1) as f64;
        let strength = (0.30 / distance).clamp(0.05, 0.30);
        draw_frame(&mut builder, scene, ghost_frame, camera, Some(strength), editor.onion.outlines);
    }

    draw_frame(&mut builder, scene, frame, camera, None, false);
}

/// Draw one frame's layers.
///
/// `ghost` fades everything for onion skinning; `None` draws normally.
fn draw_frame(
    builder: &mut SceneBuilder<'_>,
    scene: &buzz_scene::Scene,
    frame: u32,
    camera: Affine,
    ghost: Option<f64>,
    ghost_outlines: bool,
) {
    for layer in scene.layers().drawable_at(frame) {
        // Guides are authoring aids: visible on stage, never exported.
        let outline = layer.outline || (ghost.is_some() && ghost_outlines);
        let tint = outline.then_some(layer.color);
        let faded = layer.kind == LayerKind::Guide;

        for object in layer.objects_at(frame) {
            draw_object(builder, object, camera, tint, faded, ghost);
        }
    }
}

fn draw_object(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    parent: Affine,
    tint: Option<Color>,
    faded: bool,
    ghost: Option<f64>,
) {
    if !object.visible {
        return;
    }
    let world = parent * object.transform;

    match &object.kind {
        ObjectKind::Group(children) => {
            for child in children {
                draw_object(builder, child, world, tint, faded, ghost);
            }
        }
        ObjectKind::Shape(shape) => {
            let path = world * shape.path.clone();

            let adjust = |c: Color| {
                let c = if faded { fade(c) } else { c };
                match ghost {
                    Some(alpha) => c.multiply_alpha(alpha as f32),
                    None => c,
                }
            };

            // Outline view: draw the silhouette in the layer colour instead of
            // the artwork, which is what the timeline's outline column does.
            if let Some(color) = tint {
                builder.stroke_hairline(&path, adjust(color), 1.0);
                return;
            }

            if let Some(fill) = shape.fill {
                builder.fill_shape(&path, adjust(fill.color));
            }
            if let Some(stroke) = shape.stroke {
                let color = adjust(stroke.color);
                if stroke.hairline {
                    builder.stroke_hairline(&path, color, 1.0);
                } else {
                    builder.stroke_shape(&path, color, stroke.width);
                }
            }
        }
    }
}

/// Guide layers draw faintly, so they read as reference rather than artwork.
fn fade(color: Color) -> Color {
    color.multiply_alpha(0.35)
}

/// Draw the chrome over the rendered stage.
pub fn draw_chrome(ui: &mut Ui, editor: &Editor, area: egui::Rect) -> ChromeResponse {
    let mut response = ChromeResponse::default();
    let painter = ui.painter_at(area);
    let camera = &editor.camera;
    let view = &editor.view;

    // Document point to screen position within `area`.
    let to_screen = |p: Point| {
        let s = camera.doc_to_screen(p);
        egui::pos2(area.min.x + s.x as f32, area.min.y + s.y as f32)
    };

    let visible = camera.visible_doc_rect();

    if view.show_grid {
        draw_grid(&painter, area, camera, view, visible);
    }

    // The stage border sits above the grid but below artwork chrome.
    let stage = editor.scene().stage().stage_rect();
    let stage_rect = egui::Rect::from_two_pos(
        to_screen(Point::new(stage.x0, stage.y0)),
        to_screen(Point::new(stage.x1, stage.y1)),
    );
    painter.rect_stroke(
        stage_rect,
        0.0,
        Stroke::new(1.0, Palette::STAGE_BORDER),
        StrokeKind::Outside,
    );

    if view.show_guides {
        draw_guides(&painter, area, view, to_screen);
    }

    draw_selection(&painter, editor, to_screen);
    draw_preview(&painter, editor, to_screen);

    if view.show_rulers {
        response.new_guide = draw_rulers(ui, area, editor);
    }

    response.hovered = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| area.contains(p));

    response
}

fn draw_grid(
    painter: &egui::Painter,
    area: egui::Rect,
    camera: &buzz_geom::Camera,
    view: &buzz_ui::ViewSettings,
    visible: Rect,
) {
    let spacing = view.effective_grid_spacing(camera.zoom);
    if spacing <= 0.0 || !spacing.is_finite() {
        return;
    }

    // Bounded so a pathological zoom cannot try to draw millions of lines.
    const MAX_LINES: i64 = 4_000;
    let stroke = Stroke::new(1.0, Palette::GRID);

    let first_x = (visible.x0 / spacing).floor() as i64;
    let last_x = (visible.x1 / spacing).ceil() as i64;
    if last_x.saturating_sub(first_x) <= MAX_LINES {
        for i in first_x..=last_x {
            let x = camera.doc_to_screen(Point::new(i as f64 * spacing, visible.y0)).x;
            let x = area.min.x + x as f32;
            painter.line_segment([egui::pos2(x, area.min.y), egui::pos2(x, area.max.y)], stroke);
        }
    }

    let first_y = (visible.y0 / spacing).floor() as i64;
    let last_y = (visible.y1 / spacing).ceil() as i64;
    if last_y.saturating_sub(first_y) <= MAX_LINES {
        for i in first_y..=last_y {
            let y = camera.doc_to_screen(Point::new(visible.x0, i as f64 * spacing)).y;
            let y = area.min.y + y as f32;
            painter.line_segment([egui::pos2(area.min.x, y), egui::pos2(area.max.x, y)], stroke);
        }
    }
}

fn draw_guides(
    painter: &egui::Painter,
    area: egui::Rect,
    view: &buzz_ui::ViewSettings,
    to_screen: impl Fn(Point) -> egui::Pos2,
) {
    let color = if view.lock_guides {
        Palette::GUIDE_LOCKED
    } else {
        Palette::GUIDE
    };
    let stroke = Stroke::new(1.0, color);

    for guide in &view.guides {
        match guide.orientation {
            Orientation::Vertical => {
                let x = to_screen(Point::new(guide.position, 0.0)).x;
                if x >= area.min.x && x <= area.max.x {
                    painter.line_segment(
                        [egui::pos2(x, area.min.y), egui::pos2(x, area.max.y)],
                        stroke,
                    );
                }
            }
            Orientation::Horizontal => {
                let y = to_screen(Point::new(0.0, guide.position)).y;
                if y >= area.min.y && y <= area.max.y {
                    painter.line_segment(
                        [egui::pos2(area.min.x, y), egui::pos2(area.max.x, y)],
                        stroke,
                    );
                }
            }
        }
    }
}

/// Selection outline and transform handles.
fn draw_selection(
    painter: &egui::Painter,
    editor: &Editor,
    to_screen: impl Fn(Point) -> egui::Pos2,
) {
    let Some(bounds) = editor.selection.bounds(editor.scene()) else {
        return;
    };
    let rect = egui::Rect::from_two_pos(
        to_screen(Point::new(bounds.x0, bounds.y0)),
        to_screen(Point::new(bounds.x1, bounds.y1)),
    );

    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, Palette::SELECTION),
        StrokeKind::Outside,
    );

    // Subselection shows the path's anchors instead of a transform box, which
    // is what makes individual points grabbable.
    if editor.tool() == buzz_ui::ToolId::Subselection {
        const ANCHOR: f32 = 5.0;
        for anchor in editor.selected_anchors() {
            let at = to_screen(anchor.point);
            let square = egui::Rect::from_center_size(at, egui::vec2(ANCHOR, ANCHOR));
            painter.rect_filled(square, 0.0, Palette::HANDLE_FILL);
            painter.rect_stroke(
                square,
                0.0,
                Stroke::new(1.0, Palette::SELECTION),
                StrokeKind::Outside,
            );
        }
        return;
    }

    // Handles only for the tool that can use them.
    if editor.tool() != buzz_ui::ToolId::FreeTransform {
        return;
    }
    const HANDLE: f32 = 6.0;
    for corner in [
        rect.left_top(),
        rect.center_top(),
        rect.right_top(),
        rect.right_center(),
        rect.right_bottom(),
        rect.center_bottom(),
        rect.left_bottom(),
        rect.left_center(),
    ] {
        let handle = egui::Rect::from_center_size(corner, egui::vec2(HANDLE, HANDLE));
        painter.rect_filled(handle, 0.0, Palette::HANDLE_FILL);
        painter.rect_stroke(
            handle,
            0.0,
            Stroke::new(1.0, Palette::HANDLE_STROKE),
            StrokeKind::Outside,
        );
    }
}

/// Live feedback for the gesture in progress.
fn draw_preview(
    painter: &egui::Painter,
    editor: &Editor,
    to_screen: impl Fn(Point) -> egui::Pos2,
) {
    let stroke = Stroke::new(1.0, Palette::SELECTION);

    match editor.preview() {
        Preview::None => {}
        Preview::Marquee(rect) => {
            let r = egui::Rect::from_two_pos(
                to_screen(Point::new(rect.x0, rect.y0)),
                to_screen(Point::new(rect.x1, rect.y1)),
            );
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(0, 168, 255, 30));
            painter.rect_stroke(r, 0.0, stroke, StrokeKind::Outside);
        }
        Preview::Shape(path) | Preview::Stroke { path, .. } => {
            // Flattened for display only; the committed geometry keeps its
            // curves.
            let tolerance = 0.25 / editor.camera.zoom.max(f64::MIN_POSITIVE);
            let mut points: Vec<egui::Pos2> = Vec::new();
            kurbo::flatten(path.iter(), tolerance, |el| match el {
                kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => points.push(to_screen(p)),
                _ => {}
            });
            if points.len() >= 2 {
                painter.add(egui::Shape::line(points, stroke));
            }
        }
    }
}

/// Ruler strips along the top and left, and dragging guides off them.
fn draw_rulers(ui: &mut Ui, area: egui::Rect, editor: &Editor) -> Option<buzz_ui::Guide> {
    let camera = &editor.camera;
    let thickness = Metrics::RULER;
    let painter = ui.painter_at(area);

    let top = egui::Rect::from_min_size(area.min, egui::vec2(area.width(), thickness));
    let left = egui::Rect::from_min_size(area.min, egui::vec2(thickness, area.height()));

    painter.rect_filled(top, 0.0, Palette::RULER_BG);
    painter.rect_filled(left, 0.0, Palette::RULER_BG);

    let step = editor.view.ruler_step(camera.zoom);
    let visible = camera.visible_doc_rect();
    let font = FontId::proportional(9.0);
    let tick = Stroke::new(1.0, Palette::RULER_TICK);

    const MAX_TICKS: i64 = 2_000;

    let first = (visible.x0 / step).floor() as i64;
    let last = (visible.x1 / step).ceil() as i64;
    if last.saturating_sub(first) <= MAX_TICKS {
        for i in first..=last {
            let value = i as f64 * step;
            let x = area.min.x + camera.doc_to_screen(Point::new(value, 0.0)).x as f32;
            if x < area.min.x + thickness || x > area.max.x {
                continue;
            }
            painter.line_segment(
                [egui::pos2(x, top.max.y - 5.0), egui::pos2(x, top.max.y)],
                tick,
            );
            painter.text(
                egui::pos2(x + 2.0, top.min.y + 1.0),
                Align2::LEFT_TOP,
                format_ruler(value),
                font.clone(),
                Palette::RULER_TEXT,
            );
        }
    }

    let first = (visible.y0 / step).floor() as i64;
    let last = (visible.y1 / step).ceil() as i64;
    if last.saturating_sub(first) <= MAX_TICKS {
        for i in first..=last {
            let value = i as f64 * step;
            let y = area.min.y + camera.doc_to_screen(Point::new(0.0, value)).y as f32;
            if y < area.min.y + thickness || y > area.max.y {
                continue;
            }
            painter.line_segment(
                [egui::pos2(left.max.x - 5.0, y), egui::pos2(left.max.x, y)],
                tick,
            );
            painter.text(
                egui::pos2(left.min.x + 1.0, y + 1.0),
                Align2::LEFT_TOP,
                format_ruler(value),
                font.clone(),
                Palette::RULER_TEXT,
            );
        }
    }

    // Dragging off a ruler creates a guide.
    if editor.view.lock_guides {
        return None;
    }

    let mut created = None;
    let top_id = ui.id().with("ruler-top");
    let left_id = ui.id().with("ruler-left");

    let top_response = ui.interact(top, top_id, Sense::drag());
    if top_response.drag_stopped()
        && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
        && pos.y > area.min.y + thickness
    {
        let doc = camera.screen_to_doc(Point::new(
            (pos.x - area.min.x) as f64,
            (pos.y - area.min.y) as f64,
        ));
        created = Some(buzz_ui::Guide {
            position: doc.y,
            orientation: Orientation::Horizontal,
        });
    }

    let left_response = ui.interact(left, left_id, Sense::drag());
    if left_response.drag_stopped()
        && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
        && pos.x > area.min.x + thickness
    {
        let doc = camera.screen_to_doc(Point::new(
            (pos.x - area.min.x) as f64,
            (pos.y - area.min.y) as f64,
        ));
        created = Some(buzz_ui::Guide {
            position: doc.x,
            orientation: Orientation::Vertical,
        });
    }

    created
}

/// Ruler labels: plain integers where possible, scientific at extreme zoom.
fn format_ruler(value: f64) -> String {
    let magnitude = value.abs();
    if magnitude >= 1e6 {
        format!("{value:.0e}")
    } else if magnitude < 0.001 && magnitude > 0.0 {
        format!("{value:.1e}")
    } else if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::ShapeData;
    use kurbo::Rect as KRect;

    fn editor_with_art() -> Editor {
        let mut e = Editor::default();
        e.camera.viewport = buzz_geom::Size::new(1000.0, 800.0);
        let layer = e.selection.active_layer().unwrap();
        e.doc.edit("Draw", |scene| {
            scene.add_shape(
                layer,
                ShapeData::filled(
                    KRect::new(10.0, 10.0, 200.0, 150.0).to_path(1e-9),
                    Color::from_rgb8(0x33, 0x66, 0x99),
                ),
            );
        });
        e
    }

    #[test]
    fn ruler_labels_stay_short_across_the_zoom_range() {
        for value in [0.0, 1.0, -250.0, 1234.5, 1e7, 1e-6, -1e9] {
            let text = format_ruler(value);
            assert!(!text.is_empty());
            assert!(
                text.len() <= 9,
                "ruler label {text:?} is too long to fit between ticks"
            );
        }
    }

    #[test]
    fn a_scene_can_be_built_without_panicking() {
        let editor = editor_with_art();
        let mut vello = vello::Scene::new();
        build_scene(&mut vello, &editor, Rect::new(0.0, 0.0, 1000.0, 800.0));
        assert!(
            vello.encoding().n_paths > 0,
            "the stage and artwork should both encode"
        );
    }

    /// The stage must survive being drawn at any zoom, including past the
    /// point where Animate refuses to go.
    #[test]
    fn the_scene_builds_at_every_zoom_level() {
        let mut editor = editor_with_art();
        let mut vello = vello::Scene::new();

        for percent in [1.0, 100.0, 2000.0, 1e6, 1e9, 1e12] {
            editor.camera.set_zoom_percent(percent);
            build_scene(&mut vello, &editor, Rect::new(0.0, 0.0, 1000.0, 800.0));
            assert!(
                vello.encoding().n_paths > 0,
                "nothing encoded at {percent}%"
            );
        }
    }

    #[test]
    fn hidden_layers_contribute_nothing() {
        let mut editor = editor_with_art();
        let layer = editor.selection.active_layer().unwrap();

        let mut visible_scene = vello::Scene::new();
        build_scene(&mut visible_scene, &editor, Rect::new(0.0, 0.0, 800.0, 600.0));
        let with_art = visible_scene.encoding().n_paths;

        editor.doc.edit("Hide", |s| {
            s.update_layer(layer, |l| l.visible = false);
        });
        let mut hidden_scene = vello::Scene::new();
        build_scene(&mut hidden_scene, &editor, Rect::new(0.0, 0.0, 800.0, 600.0));

        assert!(
            hidden_scene.encoding().n_paths < with_art,
            "hiding a layer should reduce what is encoded"
        );
    }

    #[test]
    fn outline_mode_still_draws_something() {
        let mut editor = editor_with_art();
        let layer = editor.selection.active_layer().unwrap();
        editor.doc.edit("Outline", |s| {
            s.update_layer(layer, |l| l.outline = true);
        });

        let mut vello = vello::Scene::new();
        build_scene(&mut vello, &editor, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert!(vello.encoding().n_paths > 0);
    }

    #[test]
    fn nested_groups_are_drawn() {
        use buzz_scene::{Object, ObjectId};
        use std::sync::Arc;

        let mut editor = Editor::default();
        let layer = editor.selection.active_layer().unwrap();
        editor.doc.edit("Group", |scene| {
            let leaf = Arc::new(Object::shape(
                ObjectId(9001),
                ShapeData::filled(KRect::new(0.0, 0.0, 20.0, 20.0).to_path(1e-9), Color::WHITE),
            ));
            let group = Object::group(ObjectId(9002), vec![leaf])
                .with_transform(Affine::translate((50.0, 50.0)));
            scene.add_object(layer, group);
        });

        let mut vello = vello::Scene::new();
        build_scene(&mut vello, &editor, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert!(vello.encoding().n_paths > 1, "the group's leaf should encode");
    }

    #[test]
    fn chrome_draws_without_panicking() {
        let editor = editor_with_art();
        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        let _ = ctx.run_ui(Default::default(), |ui| {
            let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 800.0));
            let _ = draw_chrome(ui, &editor, area);
        });
    }

    /// A pathological zoom must not attempt to draw millions of grid lines.
    #[test]
    fn chrome_is_bounded_at_extreme_zoom() {
        let mut editor = editor_with_art();
        editor.view.show_grid = true;
        editor.view.show_rulers = true;

        let ctx = egui::Context::default();
        buzz_ui::theme::apply(&ctx);

        for percent in [0.001, 1.0, 1e6, 1e12] {
            editor.camera.set_zoom_percent(percent);
            let started = std::time::Instant::now();
            let _ = ctx.run_ui(Default::default(), |ui| {
                let area =
                    egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 900.0));
                let _ = draw_chrome(ui, &editor, area);
            });
            assert!(
                started.elapsed().as_millis() < 500,
                "chrome took {:?} at {percent}%",
                started.elapsed()
            );
        }
    }
}
