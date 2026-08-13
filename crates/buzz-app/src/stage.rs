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

use buzz_geom::{Point, Rect, Vec2};
use buzz_render::SceneBuilder;
use buzz_render::document::{
    DrawCache, FrameOptions, MaskDisplay, draw_frame, draw_frame_cached, draw_stage_context,
};
use buzz_ui::{Metrics, Orientation, Palette};
use egui::{Align2, Color32, FontId, Sense, Stroke, StrokeKind, Ui};

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
pub fn build_scene(vello: &mut vello::Scene, editor: &Editor, area: Rect, cache: &mut DrawCache) {
    let mut builder =
        SceneBuilder::new(vello, &editor.camera).with_viewport_offset(Vec2::new(area.x0, area.y0));

    let scene = editor.scene();

    // The stage rectangle. Everything outside it is pasteboard, which the
    // window's clear colour already provides.
    builder.fill_shape(&scene.stage().stage_rect(), scene.stage().background);

    let frame = editor.current_frame;
    // The camera is part of the document, so it transforms artwork but not the
    // stage rectangle the artwork sits on.
    let camera = scene.camera_transform(frame);

    // **Edit in place.** Inside a symbol, the scene it was opened from is drawn
    // first and then veiled: the artwork being edited has to be judged against
    // what surrounds it — a head against the shoulders it sits on — and an
    // empty grey stage says nothing about whether the drawing is right.
    //
    // Animate pales the context rather than hiding it, and the pale is the
    // point: everything solid on screen is then the thing you are editing.
    if !scene.edit_path().is_empty() {
        let context = FrameOptions {
            masks: MaskDisplay::WhenLocked,
            // Not lit: shading and cast shadows on artwork that is only there
            // for reference read as dirt on the stage.
            lit: false,
            ..FrameOptions::default()
        };
        draw_stage_context(&mut builder, scene, frame, camera, &context, cache);

        // The veil, over everything visible rather than over the stage
        // rectangle: the context runs off the edges of the stage as freely as
        // the artwork does, and a veil that stopped at the stage would leave a
        // bright ring of unveiled drawing around it.
        let seen = editor.camera.visible_doc_rect();
        builder.fill_shape(
            &seen.inflate(seen.width(), seen.height()),
            peniko::Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xC4),
        );
    }

    // Edit Multiple Frames draws the other keyframes **solid**, under the live
    // frame. Ghosting them would be onion skinning, and the difference between
    // the two modes is exactly that these drawings are the thing being edited
    // rather than a reference to work against.
    let place = scene.edit_place();
    for other in editor.multi_frames() {
        let options = FrameOptions {
            masks: MaskDisplay::WhenLocked,
            lit: true,
            place,
            ..FrameOptions::default()
        };
        draw_frame(&mut builder, scene, other, camera, &options);
    }

    // Onion skin ghosts first, so the live frame draws over them.
    for ghost_frame in editor.onion_frames() {
        let distance = ghost_frame.abs_diff(frame).max(1) as f64;
        let strength = (0.30 / distance).clamp(0.05, 0.30);
        let options = FrameOptions {
            ghost: Some(strength),
            ghost_outlines: editor.onion.outlines,
            masks: MaskDisplay::WhenLocked,
            place,
            // Ghosts are reference, not picture: shading and cast shadows on a
            // faded copy of another frame read as dirt on the stage.
            lit: false,
        };
        draw_frame(&mut builder, scene, ghost_frame, camera, &options);
    }

    // Animate shows the mask effect only once the mask layer is locked,
    // because you cannot draw inside a region you cannot see. An export
    // always masks; this is the stage.
    let live = FrameOptions {
        masks: MaskDisplay::WhenLocked,
        lit: true,
        place,
        ..FrameOptions::default()
    };
    draw_frame_cached(&mut builder, scene, frame, camera, &live, cache);

    // The brush preview is painted here, with the artwork, rather than sketched
    // as chrome. A brush stroke is the one gesture where the preview *is* the
    // result: outlining it in the selection colour would show its silhouette
    // but not its weight, and weight is the whole point of a fluid brush.
    if let Preview::Ink { path, color } = editor.preview() {
        builder.fill_shape(&path, color);
    }
}

/// Draw the chrome over the rendered stage.
pub fn draw_chrome(ui: &mut Ui, editor: &Editor, area: egui::Rect) -> ChromeResponse {
    let mut response = ChromeResponse::default();
    let painter = ui.painter_at(area);
    let camera = &editor.camera;
    let view = &editor.view;

    // Document point to screen position within `area`.
    //
    // This is the *view* — where the user has scrolled and zoomed — and nothing
    // to do with the document's camera. The stage rectangle, the rulers, the
    // grid and the guides are drawn through it, because none of them move when
    // the shot does.
    let to_screen = |p: Point| {
        let s = camera.doc_to_screen(p);
        egui::pos2(area.min.x + s.x as f32, area.min.y + s.y as f32)
    };

    // Where the *artwork* lands: the document camera, then the view.
    //
    // Selection handles, bones, warp handles and light gizmos all mark where
    // something is, so they have to travel with it. Before this they were drawn
    // straight through the view, which was already subtly wrong whenever the
    // camera panned — and glaring the moment it could tilt, because the
    // selection box sat squarely where the artwork used to be.
    //
    // A point the lens cannot see falls back to its unprojected place rather
    // than vanishing: a handle in the wrong spot is recoverable, a handle that
    // is not there at all is not.
    let shot = editor
        .scene()
        .camera_projection_at_depth(editor.current_frame, 0.0)
        .unwrap_or(buzz_geom::Projection::IDENTITY);
    // And through the place, so that inside a symbol opened *in place* the
    // handles, bones and the transformation point land on the artwork rather
    // than at the origin the artwork is no longer drawn at.
    let place = editor.scene().edit_place();
    let to_screen_art = |p: Point| {
        let p = place * p;
        to_screen(shot.map_point(p).unwrap_or(p))
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
        Stroke::new(1.0, Palette::stage_border()),
        StrokeKind::Outside,
    );

    if view.show_guides {
        draw_guides(&painter, area, view, to_screen);
    }

    draw_selection(&painter, editor, to_screen_art, to_screen);
    draw_rigs(&painter, editor, to_screen_art);
    draw_lights(&painter, editor, to_screen_art);
    draw_preview(&painter, editor, to_screen_art);

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
    let stroke = Stroke::new(1.0, Palette::grid());

    let first_x = (visible.x0 / spacing).floor() as i64;
    let last_x = (visible.x1 / spacing).ceil() as i64;
    if last_x.saturating_sub(first_x) <= MAX_LINES {
        for i in first_x..=last_x {
            let x = camera
                .doc_to_screen(Point::new(i as f64 * spacing, visible.y0))
                .x;
            let x = area.min.x + x as f32;
            painter.line_segment(
                [egui::pos2(x, area.min.y), egui::pos2(x, area.max.y)],
                stroke,
            );
        }
    }

    let first_y = (visible.y0 / spacing).floor() as i64;
    let last_y = (visible.y1 / spacing).ceil() as i64;
    if last_y.saturating_sub(first_y) <= MAX_LINES {
        for i in first_y..=last_y {
            let y = camera
                .doc_to_screen(Point::new(visible.x0, i as f64 * spacing))
                .y;
            let y = area.min.y + y as f32;
            painter.line_segment(
                [egui::pos2(area.min.x, y), egui::pos2(area.max.x, y)],
                stroke,
            );
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
        Palette::guide_locked()
    } else {
        Palette::guide()
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
    to_screen_view: impl Fn(Point) -> egui::Pos2,
) {
    let Some(bounds) = editor.selection.bounds(editor.scene()) else {
        return;
    };

    // Each selected object outlined where it is **drawn**. Under a tilted
    // camera, or on an object turned in space, that is a *quadrilateral* — and
    // a box drawn round it would be a box round something that is not there.
    //
    // Asked object by object rather than once for the selection's bounds,
    // because two objects turned different ways have no common quad.
    let mut drawn_any = false;
    for id in editor.selection.iter() {
        let Some(quad) = editor.object_quad(id) else {
            continue;
        };
        let corners = quad.map(&to_screen_view);
        let mut outline: Vec<egui::Pos2> = corners.to_vec();
        outline.push(corners[0]);
        painter.add(egui::Shape::line(
            outline,
            Stroke::new(1.0, Palette::selection()),
        ));
        drawn_any = true;
    }

    // The corners of the whole selection, for the handles below — and the
    // outline itself when nothing could be asked object by object.
    let corners = [
        Point::new(bounds.x0, bounds.y0),
        Point::new(bounds.x1, bounds.y0),
        Point::new(bounds.x1, bounds.y1),
        Point::new(bounds.x0, bounds.y1),
    ]
    .map(&to_screen);

    if !drawn_any {
        let mut outline: Vec<egui::Pos2> = corners.to_vec();
        outline.push(corners[0]);
        painter.add(egui::Shape::line(
            outline,
            Stroke::new(1.0, Palette::selection()),
        ));
    }

    // The box the handles hang off. With no tilt it is exactly the old
    // rectangle; with tilt it is the quad's extent, which is the honest
    // approximation — a transform handle on a perspective quad wants a gizmo
    // of its own, and that is recorded in §7 rather than faked here.
    let mut rect = egui::Rect::from_two_pos(corners[0], corners[2]);
    for corner in corners {
        rect = rect.union(egui::Rect::from_two_pos(corner, corner));
    }

    // Subselection shows the path's anchors instead of a transform box, which
    // is what makes individual points grabbable.
    if editor.tool() == buzz_ui::ToolId::Subselection {
        const ANCHOR: f32 = 5.0;
        for anchor in editor.selected_anchors() {
            let at = to_screen(anchor.point);
            let square = egui::Rect::from_center_size(at, egui::vec2(ANCHOR, ANCHOR));
            painter.rect_filled(square, 0.0, Palette::handle_fill());
            painter.rect_stroke(
                square,
                0.0,
                Stroke::new(1.0, Palette::selection()),
                StrokeKind::Outside,
            );
        }
        return;
    }

    // Handles only for the tool that can use them — but the transformation
    // point is drawn for the selection tools as well, because they can move it
    // (see `tools::finish_drag`), and a control you can grab and cannot see is
    // worse than one that is not there.
    if editor.tool() == buzz_ui::ToolId::GradientTransform {
        draw_gradient_handles(painter, editor, &to_screen);
        return;
    }
    if editor.tool() != buzz_ui::ToolId::FreeTransform {
        draw_pivot(painter, editor, &to_screen);
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
        painter.rect_filled(handle, 0.0, Palette::handle_fill());
        painter.rect_stroke(
            handle,
            0.0,
            Stroke::new(1.0, Palette::handle_stroke()),
            StrokeKind::Outside,
        );
    }

    // The transformation point: Animate's white circle, and what rotation,
    // skew and Alt-scale all turn about. Drawn last so it is never hidden
    // under a handle, and hollow so the artwork underneath stays visible —
    // it is usually put on a hinge or a joint, which is exactly the part you
    // need to see while placing it.
    draw_pivot(painter, editor, &to_screen);
}

/// The transformation point: Animate's white circle.
///
/// Hollow, so the artwork underneath stays visible — it is usually put on a
/// hinge or a joint, which is exactly the part you need to see while placing
/// it. While it is being dragged it is drawn under the pointer instead: the
/// move itself lands when the drag ends, and a circle that sat still until
/// then made the gesture look as though it had not taken.
fn draw_pivot(
    painter: &egui::Painter,
    editor: &Editor,
    to_screen: &impl Fn(Point) -> egui::Pos2,
) {
    let dragging = match editor.preview() {
        Preview::Pivot(at) => Some(at),
        _ => None,
    };
    let Some(pivot) = dragging.or_else(|| editor.pivot()) else {
        return;
    };
    let at = to_screen(pivot);
    painter.circle_filled(at, 4.5, Palette::handle_fill());
    painter.circle_stroke(at, 4.5, Stroke::new(1.0, Palette::handle_stroke()));
    painter.circle_filled(at, 1.5, Palette::handle_stroke());
}

/// Animate's gradient widget: the ramp drawn as a line, with a grip on each
/// end of it and one across.
///
/// **Chrome, not artwork**, like the bones below — drawn here in screen space,
/// so it can never reach an exported frame.
///
/// Each grip is a different shape rather than a different colour, because they
/// sit close together and two of them coincide on a gradient nobody has
/// adjusted: a square end, a diamond across, a triangle for the focus. Shape
/// survives being four pixels wide in a way that hue does not.
fn draw_gradient_handles(
    painter: &egui::Painter,
    editor: &Editor,
    to_screen: &impl Fn(Point) -> egui::Pos2,
) {
    let Some((handles, kind)) = editor.selected_gradient_handles() else {
        return;
    };
    let stroke = Stroke::new(1.0, Palette::handle_stroke());
    let (centre, end, across) = (
        to_screen(handles.center),
        to_screen(handles.end),
        to_screen(handles.width),
    );

    // The two axes, so the shape of the gradient is legible before anything is
    // grabbed — for a radial one this is the ellipse's two radii.
    painter.line_segment([centre, end], stroke);
    painter.line_segment([centre, across], Stroke::new(1.0, Palette::handle_fill()));

    // The centre: a hollow circle, as the transformation point is, because it
    // means the same thing — the point everything else is measured from.
    painter.circle_filled(centre, 4.5, Palette::handle_fill());
    painter.circle_stroke(centre, 4.5, stroke);

    let square = egui::Rect::from_center_size(end, egui::vec2(7.0, 7.0));
    painter.rect_filled(square, 0.0, Palette::handle_fill());
    painter.rect_stroke(square, 0.0, stroke, StrokeKind::Outside);

    let diamond = egui::Shape::convex_polygon(
        vec![
            egui::pos2(across.x, across.y - 5.0),
            egui::pos2(across.x + 5.0, across.y),
            egui::pos2(across.x, across.y + 5.0),
            egui::pos2(across.x - 5.0, across.y),
        ],
        Palette::handle_fill(),
        stroke,
    );
    painter.add(diamond);

    // The focus belongs to a radial gradient only: a linear ramp has no centre
    // for a hot spot to sit off.
    if kind == buzz_scene::GradientKind::Radial {
        let f = to_screen(handles.focus);
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(f.x, f.y - 6.0),
                egui::pos2(f.x + 5.0, f.y + 4.0),
                egui::pos2(f.x - 5.0, f.y + 4.0),
            ],
            Palette::handle_fill(),
            stroke,
        ));
    }
}

/// Live feedback for the gesture in progress.
/// Draw armatures and warp handles over the artwork.
///
/// **Chrome, not artwork** — bones are drawn here, with egui, in screen space,
/// so they are never part of an exported frame. A skeleton visible in the
/// finished animation would be an unmistakable bug, and keeping it on this
/// side of the split makes that impossible rather than merely unlikely.
///
/// Bones are drawn as Animate draws them: a tapered quadrilateral, widest a
/// quarter of the way along, so which end is the head can be read at a glance.
fn draw_rigs(painter: &egui::Painter, editor: &Editor, to_screen: impl Fn(Point) -> egui::Pos2) {
    let frame = editor.current_frame;
    let scene = editor.scene();
    let selected = |id: buzz_scene::ObjectId| editor.selection.contains(id);

    for (object, segments) in crate::rigging::stage_segments(scene, frame) {
        let colour = if selected(object) {
            Palette::selection()
        } else {
            Color32::from_rgba_unmultiplied(255, 210, 90, 170)
        };

        for (head, tip) in segments {
            let outline = crate::rigging::bone_outline(head, tip, bone_width(editor, head, tip));
            let points: Vec<egui::Pos2> = outline.iter().map(|p| to_screen(*p)).collect();
            painter.add(egui::Shape::convex_polygon(
                points,
                Color32::from_rgba_unmultiplied(255, 210, 90, 60),
                Stroke::new(1.0, colour),
            ));
            // The joint at the head, which is what the bone turns about.
            painter.circle_stroke(to_screen(head), 3.0, Stroke::new(1.0, colour));
        }
    }

    for (object, handles) in crate::rigging::stage_handles(scene, frame) {
        let colour = if selected(object) {
            Palette::selection()
        } else {
            Color32::from_rgba_unmultiplied(120, 220, 255, 200)
        };
        for (at, moved) in handles {
            let screen = to_screen(at);
            // A moved handle is filled, an untouched one hollow, so it is
            // obvious at a glance which handles are holding the deformation.
            if moved {
                painter.circle_filled(screen, 4.0, colour);
            } else {
                painter.circle_stroke(screen, 4.0, Stroke::new(1.5, colour));
            }
        }
    }

    // The bone being dragged out, before it exists in the document.
    if let Some((head, current)) = editor.rig_preview() {
        painter.line_segment(
            [to_screen(head), to_screen(current)],
            Stroke::new(1.5, Palette::selection()),
        );
        painter.circle_stroke(to_screen(head), 3.0, Stroke::new(1.0, Palette::selection()));
    }
}

/// The light handles, drawn as chrome so they can never be exported.
///
/// A sun is a handle on a dial about the middle of the stage: which way round
/// it sits is the direction the light comes from, and how far out it sits is
/// how low the sun is. The dark spoke opposite the handle is where the shadow
/// falls — the thing the animator is really choosing — so the gizmo shows the
/// consequence rather than the angle.
///
/// A lamp is drawn where it is, inside a ring showing its reach.
fn draw_lights(painter: &egui::Painter, editor: &Editor, to_screen: impl Fn(Point) -> egui::Pos2) {
    if !editor.light_panel.gizmos {
        return;
    }

    if !editor.scene().lights().enabled {
        // Handles for a rig that is switched off would promise an effect the
        // document is not applying.
        return;
    }

    for gizmo in crate::lights::gizmos(editor.scene()) {
        let [r, g, b, _] = gizmo.color.to_rgba8().to_u8_array();
        // A switched-off light still shows, dimmed: a handle that vanished
        // would leave a light nobody can find again.
        let alpha = if gizmo.enabled { 255 } else { 110 };
        let colour = Color32::from_rgba_unmultiplied(r, g, b, alpha);
        // Structure is drawn in ink, not in the light's own colour. A warm key
        // is very nearly white, and a near-white gizmo on a white stage is
        // invisible — which is exactly how the first version of this looked.
        let ink = Color32::from_rgba_unmultiplied(0, 0, 0, alpha);
        let ghost = Color32::from_rgba_unmultiplied(0, 0, 0, alpha / 5);
        let selected = editor.light_panel.selected == Some(gizmo.id);
        let ring = if selected { Palette::selection() } else { ink };

        match gizmo.kind {
            crate::lights::GizmoKind::Sun {
                centre,
                dial,
                handle,
                shadow,
            } => {
                let middle = to_screen(centre);
                let rim = to_screen(centre + Vec2::new(dial, 0.0));
                painter.circle_stroke(middle, rim.x - middle.x, Stroke::new(1.0, ghost));

                // Where the shadow runs, and how far. Drawn heavier than the
                // arm towards the light because it is the consequence the
                // animator is actually choosing.
                painter.line_segment([middle, to_screen(shadow)], Stroke::new(2.0, ink));
                painter.line_segment([middle, to_screen(handle)], Stroke::new(1.0, ghost));

                let at = to_screen(handle);
                let size = if selected { 7.0 } else { 5.5 };
                painter.circle_filled(at, size, colour);
                painter.circle_stroke(at, size, Stroke::new(1.5, ring));

                // Rays, so a sun is not mistaken for a lamp at a glance.
                for step in 0..8 {
                    let angle = step as f32 * std::f32::consts::TAU / 8.0;
                    let dir = egui::vec2(angle.cos(), angle.sin());
                    painter.line_segment(
                        [at + dir * (size + 3.0), at + dir * (size + 6.0)],
                        Stroke::new(1.5, ink),
                    );
                }
            }

            crate::lights::GizmoKind::Lamp { at, radius } => {
                let middle = to_screen(at);
                let edge = to_screen(at + Vec2::new(radius, 0.0));
                // The reach ring, which is also a drag handle.
                painter.circle_stroke(middle, edge.x - middle.x, Stroke::new(1.0, ghost));

                let size = if selected { 6.0 } else { 5.0 };
                painter.circle_filled(middle, size, colour);
                painter.circle_stroke(middle, size, Stroke::new(1.5, ink));
                painter.circle_stroke(middle, size + 4.0, Stroke::new(1.0, ring));
            }
        }
    }
}

/// How wide to draw a bone: a fraction of its length, bounded in *screen*
/// pixels so a long bone is not a wedge and a short one is still visible.
fn bone_width(editor: &Editor, head: Point, tip: Point) -> f64 {
    let zoom = editor.camera.zoom.max(f64::MIN_POSITIVE);
    let length = (tip - head).hypot();
    (length * 0.12).clamp(2.0 / zoom, 14.0 / zoom)
}

fn draw_preview(painter: &egui::Painter, editor: &Editor, to_screen: impl Fn(Point) -> egui::Pos2) {
    let stroke = Stroke::new(1.0, Palette::selection());

    match editor.preview() {
        Preview::None => {}
        // Painted into the Vello scene by `build_scene`, in its real colour.
        Preview::Ink { .. } => {}
        // Drawn with the transform gizmo, where the circle belongs.
        Preview::Pivot(_) => {}
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

    painter.rect_filled(top, 0.0, Palette::ruler_bg());
    painter.rect_filled(left, 0.0, Palette::ruler_bg());

    let step = editor.view.ruler_step(camera.zoom);
    let visible = camera.visible_doc_rect();
    let font = FontId::proportional(9.0);
    let tick = Stroke::new(1.0, Palette::ruler_tick());

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
                Palette::ruler_text(),
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
                Palette::ruler_text(),
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
    use buzz_geom::{Affine, Shape as _};
    use buzz_scene::ShapeData;
    use kurbo::Rect as KRect;
    use peniko::Color;

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
        build_scene(
            &mut vello,
            &editor,
            Rect::new(0.0, 0.0, 1000.0, 800.0),
            &mut DrawCache::new(),
        );
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
            build_scene(
                &mut vello,
                &editor,
                Rect::new(0.0, 0.0, 1000.0, 800.0),
                &mut DrawCache::new(),
            );
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
        build_scene(
            &mut visible_scene,
            &editor,
            Rect::new(0.0, 0.0, 800.0, 600.0),
            &mut DrawCache::new(),
        );
        let with_art = visible_scene.encoding().n_paths;

        editor.doc.edit("Hide", |s| {
            s.update_layer(layer, |l| l.visible = false);
        });
        let mut hidden_scene = vello::Scene::new();
        build_scene(
            &mut hidden_scene,
            &editor,
            Rect::new(0.0, 0.0, 800.0, 600.0),
            &mut DrawCache::new(),
        );

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
        build_scene(
            &mut vello,
            &editor,
            Rect::new(0.0, 0.0, 800.0, 600.0),
            &mut DrawCache::new(),
        );
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
        build_scene(
            &mut vello,
            &editor,
            Rect::new(0.0, 0.0, 800.0, 600.0),
            &mut DrawCache::new(),
        );
        assert!(
            vello.encoding().n_paths > 1,
            "the group's leaf should encode"
        );
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
