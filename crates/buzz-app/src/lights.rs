//! On-stage light gizmos: where a light is drawn, and what dragging it does.
//!
//! # Why a sun has a position at all
//!
//! A sun has no position — it is a direction, the same everywhere on the
//! stage. But a direction is unusable as a *handle*: there is nothing to grab.
//! So the sun is drawn as Blender draws its own light gizmo in the viewport —
//! a handle out on a dial centred on the stage — and the dial's geometry
//! carries both numbers at once:
//!
//! * **Which way round** the handle sits is the azimuth.
//! * **How far out** it sits is the elevation: at the rim the sun is on the
//!   horizon and shadows run long; at the middle it is straight overhead and
//!   they collapse to nothing.
//!
//! That is the same mapping a fisheye photograph of the sky uses, and it makes
//! the one gesture an animator actually wants — "put the sun over there" —
//! into one drag rather than two sliders and a guess.
//!
//! A lamp *does* have a position, so its handle is simply where it is, with a
//! ring showing its reach that can be dragged to widen it. A sky has neither
//! direction nor position and gets no gizmo, because a handle that did nothing
//! would be worse than none.
//!
//! A **gloom** has both, and its gizmo is the one place its shape can be seen
//! at all: everywhere else it is a fade with no edge, which is the point of it
//! and also what makes it impossible to aim blind. So the wall is drawn as the
//! bar it is, with the throw running out of it to a second handle at the far
//! end — grab the bar to carry the wall, grab the far handle to swing it round
//! and set how far it reaches, which are the two numbers in one gesture again.
//!
//! # These are chrome
//!
//! Everything here is drawn by the stage overlay, never by the renderer, so a
//! light gizmo can no more appear in an exported frame than a selection
//! rectangle can.

use buzz_geom::{Point, Rect, Vec2};
use buzz_scene::{LightId, LightKind, Scene};
use peniko::Color;

/// How close a click must come to a handle, in screen pixels.
pub const GRAB_PX: f64 = 9.0;

/// The sun's dial, as a fraction of the smaller side of the stage.
///
/// Big enough that the elevation has room to be set precisely, small enough
/// that the handle stays on a stage the user has framed.
const DIAL: f64 = 0.42;

/// Straight up. The sun cannot be pushed past it, and an elevation of exactly
/// zero would put it on the horizon with an infinitely long shadow.
const MAX_ELEVATION: f64 = 1.55;
const MIN_ELEVATION: f64 = 0.03;

/// A light gizmo, in document space, ready to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gizmo {
    pub id: LightId,
    pub kind: GizmoKind,
    /// The light's own colour, so a warm key reads warm on the stage too.
    pub color: Color,
    /// A switched-off light still shows, faintly: it is easier to find a dim
    /// handle than to remember that a light exists at all.
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GizmoKind {
    /// A sun: the dial it turns on, and where its handle sits on it.
    Sun {
        centre: Point,
        dial: f64,
        handle: Point,
        /// Where the shadow runs, from the centre of the dial. Drawn because
        /// this — not the angle — is what the animator is choosing.
        shadow: Point,
    },
    /// A lamp: where it is, and how far it reaches.
    Lamp { at: Point, radius: f64 },
    /// A gloom: where its wall stands, which way it throws, how far, and how
    /// wide the wall is.
    Gloom {
        edge: Point,
        facing: Vec2,
        throw: f64,
        width: f64,
    },
}

impl Gizmo {
    /// The point a drag grabs.
    pub fn handle(&self) -> Point {
        match self.kind {
            GizmoKind::Sun { handle, .. } => handle,
            GizmoKind::Lamp { at, .. } => at,
            GizmoKind::Gloom { edge, .. } => edge,
        }
    }

    /// The far end of a gloom's throw: the second handle, where the dark has
    /// faded to nothing. `None` for anything else.
    pub fn far_handle(&self) -> Option<Point> {
        match self.kind {
            GizmoKind::Gloom {
                edge, facing, throw, ..
            } => Some(edge + facing * throw),
            _ => None,
        }
    }
}

/// A light drag in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LightGesture {
    /// Aiming a sun on its dial: both azimuth and elevation follow the point.
    Aim { light: LightId },
    /// Moving a lamp. `grab` is the offset from the handle to where the
    /// pointer took hold, so the lamp does not jump to the cursor.
    Move { light: LightId, grab: Vec2 },
    /// Widening a lamp's reach by dragging its ring.
    Reach { light: LightId },
    /// Swinging a gloom by its far handle, which sets the bearing it throws
    /// along and how far it reaches together.
    Throw { light: LightId },
}

impl LightGesture {
    pub fn light(&self) -> LightId {
        match self {
            LightGesture::Aim { light }
            | LightGesture::Move { light, .. }
            | LightGesture::Reach { light }
            | LightGesture::Throw { light } => *light,
        }
    }

    /// The undo label for this drag, so the history reads as what was done.
    pub fn label(&self) -> &'static str {
        match self {
            LightGesture::Aim { .. } => "Aim Light",
            LightGesture::Move { .. } => "Move Light",
            LightGesture::Reach { .. } => "Light Reach",
            LightGesture::Throw { .. } => "Aim Gloom",
        }
    }
}

/// The dial every sun in this document turns on.
fn dial_of(stage: Rect) -> (Point, f64) {
    let radius = stage.width().min(stage.height()) * DIAL;
    // A stage with no size still has to give a usable dial, or the sun's
    // handle lands on the centre and can never be grabbed.
    (stage.center(), radius.max(1.0))
}

/// Where a sun's handle sits on the dial.
pub fn sun_handle(stage: Rect, azimuth: f64, elevation: f64) -> Point {
    let (centre, radius) = dial_of(stage);
    let out = 1.0 - (elevation / std::f64::consts::FRAC_PI_2).clamp(0.0, 1.0);
    let (sin_a, cos_a) = azimuth.sin_cos();
    centre + Vec2::new(cos_a, sin_a) * (radius * out)
}

/// Every gizmo in the document, in draw order.
pub fn gizmos(scene: &Scene) -> Vec<Gizmo> {
    let stage = scene.stage().stage_rect();
    let (centre, dial) = dial_of(stage);

    scene
        .lights()
        .lights
        .iter()
        .filter_map(|light| {
            let kind = match light.kind {
                LightKind::Sun { azimuth, elevation } => {
                    let handle = sun_handle(stage, azimuth, elevation);
                    GizmoKind::Sun {
                        centre,
                        dial,
                        handle,
                        // The shadow runs opposite the light, and as far as
                        // the handle is out — so a low sun shows a long spoke.
                        shadow: centre - (handle - centre),
                    }
                }
                LightKind::Lamp {
                    position, radius, ..
                } => GizmoKind::Lamp {
                    at: position,
                    radius,
                },
                LightKind::Gloom {
                    edge,
                    facing,
                    throw,
                    width,
                } => {
                    let (sin_f, cos_f) = facing.sin_cos();
                    GizmoKind::Gloom {
                        edge,
                        facing: Vec2::new(cos_f, sin_f),
                        throw,
                        width,
                    }
                }
                // A sky has neither direction nor position.
                LightKind::Sky { .. } => return None,
            };

            Some(Gizmo {
                id: light.id,
                kind,
                color: light.color,
                enabled: light.enabled,
            })
        })
        .collect()
}

/// What a press at `point` would grab, if anything.
///
/// Handles first, rings second: a lamp whose ring passes near its own handle
/// must still be movable, and moving is the commoner intent.
pub fn target_at(
    scene: &Scene,
    point: Point,
    tolerance: f64,
    selected: Option<LightId>,
) -> Option<LightGesture> {
    let all = gizmos(scene);

    let nearest = all
        .iter()
        .filter(|g| (g.handle() - point).hypot() <= tolerance)
        .min_by(|a, b| {
            (a.handle() - point)
                .hypot()
                .total_cmp(&(b.handle() - point).hypot())
        });

    if let Some(gizmo) = nearest {
        return Some(match gizmo.kind {
            GizmoKind::Sun { .. } => LightGesture::Aim { light: gizmo.id },
            GizmoKind::Lamp { at, .. } => LightGesture::Move {
                light: gizmo.id,
                grab: at - point,
            },
            GizmoKind::Gloom { edge, .. } => LightGesture::Move {
                light: gizmo.id,
                grab: edge - point,
            },
        });
    }

    // **The selected lamp is grabbable anywhere inside its ring.**
    //
    // The report: "I cannot move the actual light source; the source stays in
    // place and only the outline moves." That is precisely what happens when
    // the *ring* is what gets grabbed — it resizes, the lamp does not move, and
    // the only thing that changes on screen is the outline.
    //
    // And the ring is what any hand would grab. The lamp itself was a dot nine
    // screen pixels across; the ring is a circle hundreds of units wide, drawn
    // in the light's own colour, and it is plainly the lamp on the stage. Two
    // targets, one of them a hundred times the size of the other, and the small
    // one is the one that does the thing everybody wants.
    //
    // So the whole disc moves the lamp — but only for the lamp that is
    // *selected* in the panel, because a lamp's ring covers a good part of the
    // stage and swallowing every brush stroke under it would be a far worse
    // bug than the one being fixed. Select it, then drag it, which is how
    // Blender's viewport works too.
    if let Some(inside) = all.iter().find(|gizmo| {
        Some(gizmo.id) == selected
            && match gizmo.kind {
                // Not within a hair of the ring: that is the reach handle, and
                // it has to stay reachable on the light you are working on.
                GizmoKind::Lamp { at, radius } => {
                    let distance = (point - at).hypot();
                    distance < radius - tolerance
                }
                _ => false,
            }
    }) {
        let GizmoKind::Lamp { at, .. } = inside.kind else {
            unreachable!("filtered to lamps above")
        };
        return Some(LightGesture::Move {
            light: inside.id,
            grab: at - point,
        });
    }

    // The second handles: a lamp's reach ring, and a gloom's far end. Both are
    // grabbed only once nothing nearer wanted the press, because moving is the
    // commoner intent and the two can sit close together on a short throw.
    all.iter().find_map(|gizmo| match gizmo.kind {
        GizmoKind::Lamp { at, radius } => {
            let distance = (point - at).hypot();
            ((distance - radius).abs() <= tolerance)
                .then_some(LightGesture::Reach { light: gizmo.id })
        }
        GizmoKind::Gloom { .. } => gizmo
            .far_handle()
            .filter(|far| (*far - point).hypot() <= tolerance)
            .map(|_| LightGesture::Throw { light: gizmo.id }),
        GizmoKind::Sun { .. } => None,
    })
}

/// Carry a drag to `point`.
///
/// Returns whether anything changed, so a drag that lands where it started
/// does not fill the undo history with steps that do nothing.
pub fn drag(scene: &mut Scene, gesture: LightGesture, point: Point) -> bool {
    let stage = scene.stage().stage_rect();
    let (centre, dial) = dial_of(stage);

    let Some(existing) = scene.lights().get(gesture.light()).map(|l| l.kind) else {
        return false;
    };

    let updated = match (gesture, existing) {
        (LightGesture::Aim { .. }, LightKind::Sun { .. }) => {
            let offset = point - centre;
            // Dead centre has no direction to read; leave the sun as it is
            // rather than snapping it to an arbitrary bearing.
            if offset.hypot() < 1e-6 {
                return false;
            }
            let out = (offset.hypot() / dial).clamp(0.0, 1.0);
            LightKind::Sun {
                azimuth: offset.y.atan2(offset.x),
                elevation: ((1.0 - out) * std::f64::consts::FRAC_PI_2)
                    .clamp(MIN_ELEVATION, MAX_ELEVATION),
            }
        }

        (LightGesture::Move { grab, .. }, LightKind::Lamp { height, radius, .. }) => {
            LightKind::Lamp {
                position: point + grab,
                height,
                radius,
            }
        }

        (
            LightGesture::Reach { .. },
            LightKind::Lamp {
                position, height, ..
            },
        ) => LightKind::Lamp {
            position,
            height,
            // Never zero: a lamp with no reach lights nothing and leaves a
            // ring that cannot be grabbed to undo the mistake.
            radius: (point - position).hypot().max(8.0),
        },

        (
            LightGesture::Move { grab, .. },
            LightKind::Gloom {
                facing,
                throw,
                width,
                ..
            },
        ) => LightKind::Gloom {
            edge: point + grab,
            facing,
            throw,
            width,
        },

        (
            LightGesture::Throw { .. },
            LightKind::Gloom { edge, width, .. },
        ) => {
            let out = point - edge;
            // On top of the wall there is no bearing to read; leaving the
            // gloom as it was is better than snapping it to an arbitrary one.
            if out.hypot() < 1e-6 {
                return false;
            }
            LightKind::Gloom {
                edge,
                facing: out.y.atan2(out.x),
                // Never nothing: a throw of zero is a wall with no fade, which
                // draws as nothing and leaves no handle to take it back.
                throw: out.hypot().max(20.0),
                width,
            }
        }

        // The light changed kind underneath the drag — from a script, or an
        // undo landing mid-gesture. Dropping the drag is the honest answer.
        _ => return false,
    };

    if updated == existing {
        return false;
    }

    scene
        .lights_mut()
        .get_mut(gesture.light())
        .expect("the light was found a moment ago")
        .kind = updated;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::LightKind;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn scene_with_gloom() -> (Scene, LightId) {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::gloom(Point::new(-200.0, 100.0)));
        (scene, id)
    }

    fn gloom_of(scene: &Scene, id: LightId) -> (Point, f64, f64) {
        match scene.lights().get(id).expect("the gloom").kind {
            LightKind::Gloom {
                edge, facing, throw, ..
            } => (edge, facing, throw),
            other => panic!("not a gloom: {other:?}"),
        }
    }

    /// **The far handle swings the wall and sets how far it reaches**, in one
    /// drag — the same bargain the sun's dial makes, and for the same reason:
    /// what the animator is choosing is where the darkness gets to, not a
    /// bearing in radians and a length in pixels.
    #[test]
    fn a_glooms_far_handle_swings_it_and_sets_the_throw() {
        let (mut scene, id) = scene_with_gloom();
        let (edge, _, _) = gloom_of(&scene, id);

        // Straight down from the wall, three hundred units out.
        let target = edge + Vec2::new(0.0, 300.0);
        assert!(drag(&mut scene, LightGesture::Throw { light: id }, target));

        let (moved, facing, throw) = gloom_of(&scene, id);
        assert_eq!(moved, edge, "aiming must not carry the wall with it");
        assert!((facing - FRAC_PI_2).abs() < 1e-9, "{facing}");
        assert!((throw - 300.0).abs() < 1e-9, "{throw}");
    }

    /// Dragging the far handle onto the wall itself has no bearing to read.
    /// Holding the gloom as it was is what the user can see and undo; a NaN
    /// out of a zero-length vector is not.
    #[test]
    fn a_gloom_aimed_at_its_own_wall_does_not_move() {
        let (mut scene, id) = scene_with_gloom();
        let before = gloom_of(&scene, id);
        assert!(!drag(
            &mut scene,
            LightGesture::Throw { light: id },
            before.0
        ));
        assert_eq!(gloom_of(&scene, id), before);
    }

    /// The wall is carried by its own handle, and keeps its grip so it does
    /// not jump to the cursor — the same rule a lamp follows.
    #[test]
    fn a_gloom_is_carried_by_its_wall() {
        let (mut scene, id) = scene_with_gloom();
        let (edge, facing, throw) = gloom_of(&scene, id);

        let grabbed = edge + Vec2::new(3.0, -2.0);
        let gesture = target_at(&scene, grabbed, GRAB_PX, None).expect("the wall is grabbable");
        assert!(matches!(gesture, LightGesture::Move { .. }));

        assert!(drag(&mut scene, gesture, grabbed + Vec2::new(40.0, 10.0)));
        let (moved, still_facing, still_throw) = gloom_of(&scene, id);
        assert_eq!(moved, edge + Vec2::new(40.0, 10.0));
        assert_eq!((still_facing, still_throw), (facing, throw));
    }

    fn scene_with_sun() -> (Scene, LightId) {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::Sun {
            azimuth: 0.0,
            elevation: 0.6,
        });
        (scene, id)
    }

    fn sun_of(scene: &Scene, id: LightId) -> (f64, f64) {
        match scene.lights().get(id).expect("the sun").kind {
            LightKind::Sun { azimuth, elevation } => (azimuth, elevation),
            other => panic!("not a sun: {other:?}"),
        }
    }

    /// **The report: "I cannot move the actual light source; the source stays
    /// in place and only the outline moves."**
    ///
    /// The ring is the only target on the stage big enough to aim at, so it is
    /// what gets grabbed — and grabbing the ring resizes it. The lamp does not
    /// move, and the one thing that changes is the outline.
    #[test]
    fn a_selected_lamp_is_dragged_by_its_ring_rather_than_resized() {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::lamp(Point::new(200.0, 200.0)));
        let LightKind::Lamp { radius, .. } = scene.lights().get(id).expect("the lamp").kind else {
            panic!("not a lamp")
        };

        // Well inside the ring, where a hand aiming at the lamp lands.
        let inside = Point::new(200.0 + radius * 0.6, 200.0);
        let gesture = target_at(&scene, inside, GRAB_PX, Some(id)).expect("the lamp is grabbable");
        assert!(
            matches!(gesture, LightGesture::Move { .. }),
            "grabbing inside the ring must move the lamp, not resize it: {gesture:?}"
        );

        assert!(drag(&mut scene, gesture, inside + Vec2::new(60.0, 20.0)));
        match scene.lights().get(id).expect("the lamp").kind {
            LightKind::Lamp { position, radius: r, .. } => {
                assert_eq!(position, Point::new(260.0, 220.0), "the lamp must move");
                assert_eq!(r, radius, "and its reach must not change");
            }
            other => panic!("not a lamp: {other:?}"),
        }
    }

    /// The reach handle survives it: the ring's own edge still resizes, so the
    /// one gesture that was working is not traded for the one that was not.
    #[test]
    fn the_edge_of_a_selected_lamps_ring_still_sets_its_reach() {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::lamp(Point::new(200.0, 200.0)));
        let LightKind::Lamp { radius, .. } = scene.lights().get(id).expect("the lamp").kind else {
            panic!("not a lamp")
        };

        let on_ring = Point::new(200.0 + radius, 200.0);
        assert_eq!(
            target_at(&scene, on_ring, GRAB_PX, Some(id)),
            Some(LightGesture::Reach { light: id }),
            "the ring's edge is still the reach handle"
        );
    }

    /// **And an unselected lamp does not swallow the stage.** A ring covers a
    /// good part of the picture, and a brush stroke aimed at the artwork under
    /// one must reach the artwork.
    #[test]
    fn an_unselected_lamps_ring_does_not_swallow_the_stage() {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::lamp(Point::new(200.0, 200.0)));
        let LightKind::Lamp { radius, .. } = scene.lights().get(id).expect("the lamp").kind else {
            panic!("not a lamp")
        };
        let inside = Point::new(200.0 + radius * 0.6, 200.0);

        assert_eq!(
            target_at(&scene, inside, GRAB_PX, None),
            None,
            "with nothing selected the ring is not a drag target"
        );
        assert_eq!(
            target_at(&scene, inside, GRAB_PX, Some(LightId(999))),
            None,
            "and neither is another light's ring"
        );
    }

    #[test]
    fn a_sky_has_no_gizmo_because_it_has_nowhere_to_be() {
        let mut scene = Scene::default();
        scene.add_light(LightKind::sky());
        assert!(gizmos(&scene).is_empty());
    }

    /// The handle is a faithful picture of the light: put the sun overhead and
    /// the handle is at the centre; drop it to the horizon and it is at the rim.
    #[test]
    fn the_handle_reads_the_elevation_off_the_dial() {
        let stage = Rect::new(0.0, 0.0, 550.0, 400.0);
        let (centre, dial) = dial_of(stage);

        let overhead = sun_handle(stage, 0.0, FRAC_PI_2);
        assert!(
            (overhead - centre).hypot() < 1e-9,
            "an overhead sun belongs at the centre"
        );

        let horizon = sun_handle(stage, 0.0, 0.0);
        assert!(
            ((horizon - centre).hypot() - dial).abs() < 1e-9,
            "a sun on the horizon belongs on the rim"
        );
    }

    /// Round-trip: drag the handle somewhere, and the light now points there.
    #[test]
    fn aiming_puts_the_sun_where_it_was_dragged() {
        let (mut scene, id) = scene_with_sun();
        let stage = scene.stage().stage_rect();
        let (centre, dial) = dial_of(stage);

        // Half way out, to the left: azimuth pi, elevation half of straight up.
        let target = centre + Vec2::new(-dial * 0.5, 0.0);
        assert!(drag(&mut scene, LightGesture::Aim { light: id }, target));

        let (azimuth, elevation) = sun_of(&scene, id);
        assert!((azimuth.abs() - PI).abs() < 1e-9, "azimuth {azimuth}");
        assert!(
            (elevation - FRAC_PI_2 * 0.5).abs() < 1e-9,
            "elevation {elevation}"
        );

        // And the handle comes back to where the pointer was.
        let back = sun_handle(stage, azimuth, elevation);
        assert!((back - target).hypot() < 1e-6, "{back:?} vs {target:?}");
    }

    /// Dragging past the rim keeps the sun on the horizon rather than letting
    /// the elevation go negative and the shadow flip inside out.
    #[test]
    fn the_sun_cannot_be_dragged_below_the_horizon_or_past_the_zenith() {
        let (mut scene, id) = scene_with_sun();
        let (centre, dial) = dial_of(scene.stage().stage_rect());

        drag(
            &mut scene,
            LightGesture::Aim { light: id },
            centre + Vec2::new(dial * 40.0, 0.0),
        );
        let (_, low) = sun_of(&scene, id);
        assert!((MIN_ELEVATION..=MAX_ELEVATION).contains(&low), "{low}");

        drag(
            &mut scene,
            LightGesture::Aim { light: id },
            centre + Vec2::new(1e-4, 0.0),
        );
        let (_, high) = sun_of(&scene, id);
        assert!(high <= MAX_ELEVATION, "{high}");
    }

    #[test]
    fn a_drag_that_changes_nothing_reports_nothing() {
        let (mut scene, id) = scene_with_sun();
        let stage = scene.stage().stage_rect();
        let (azimuth, elevation) = sun_of(&scene, id);
        let handle = sun_handle(stage, azimuth, elevation);

        assert!(
            !drag(&mut scene, LightGesture::Aim { light: id }, handle),
            "dragging the handle to where it already is is not an edit"
        );
    }

    #[test]
    fn a_lamp_keeps_its_grip_so_it_does_not_jump_to_the_cursor() {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::lamp(Point::new(100.0, 100.0)));

        // Grabbed 6px off the middle of the handle.
        let grabbed = Point::new(106.0, 100.0);
        let gesture = target_at(&scene, grabbed, GRAB_PX, None).expect("the lamp");
        assert_eq!(gesture.light(), id);

        assert!(drag(&mut scene, gesture, Point::new(306.0, 200.0)));
        match scene.lights().get(id).expect("the lamp").kind {
            LightKind::Lamp { position, .. } => {
                assert_eq!(position, Point::new(300.0, 200.0), "the grip was lost");
            }
            other => panic!("not a lamp: {other:?}"),
        }
    }

    #[test]
    fn a_lamps_ring_sets_its_reach() {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::lamp(Point::new(200.0, 200.0)));
        let radius = match scene.lights().get(id).unwrap().kind {
            LightKind::Lamp { radius, .. } => radius,
            other => panic!("{other:?}"),
        };

        // A press on the ring, well away from the handle in the middle.
        let on_ring = Point::new(200.0 + radius, 200.0);
        assert_eq!(
            target_at(&scene, on_ring, GRAB_PX, None),
            Some(LightGesture::Reach { light: id })
        );

        drag(
            &mut scene,
            LightGesture::Reach { light: id },
            Point::new(500.0, 200.0),
        );
        match scene.lights().get(id).unwrap().kind {
            LightKind::Lamp { radius, .. } => assert_eq!(radius, 300.0),
            other => panic!("{other:?}"),
        }
    }

    /// Reach can be pulled in, but not to nothing — a ring of zero radius
    /// could never be grabbed again.
    #[test]
    fn a_lamps_reach_never_collapses() {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::lamp(Point::new(200.0, 200.0)));
        drag(
            &mut scene,
            LightGesture::Reach { light: id },
            Point::new(200.0, 200.0),
        );
        match scene.lights().get(id).unwrap().kind {
            LightKind::Lamp { radius, .. } => assert!(radius >= 8.0, "{radius}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_stage_holds_nothing_to_grab() {
        let (scene, _) = scene_with_sun();
        assert_eq!(
            target_at(&scene, Point::new(-4000.0, -4000.0), GRAB_PX, None),
            None
        );
    }

    /// A light deleted between the press and the move must not panic the drag.
    #[test]
    fn a_drag_survives_its_light_disappearing() {
        let (mut scene, id) = scene_with_sun();
        scene.lights_mut().remove(id);
        assert!(!drag(
            &mut scene,
            LightGesture::Aim { light: id },
            Point::new(10.0, 10.0)
        ));
    }

    /// The kind changed underneath the gesture: a sun drag must not silently
    /// rewrite a lamp.
    #[test]
    fn a_mismatched_gesture_does_nothing() {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::lamp(Point::new(10.0, 10.0)));
        assert!(!drag(
            &mut scene,
            LightGesture::Aim { light: id },
            Point::new(90.0, 90.0)
        ));
    }
}
