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
}

impl Gizmo {
    /// The point a drag grabs.
    pub fn handle(&self) -> Point {
        match self.kind {
            GizmoKind::Sun { handle, .. } => handle,
            GizmoKind::Lamp { at, .. } => at,
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
}

impl LightGesture {
    pub fn light(&self) -> LightId {
        match self {
            LightGesture::Aim { light }
            | LightGesture::Move { light, .. }
            | LightGesture::Reach { light } => *light,
        }
    }

    /// The undo label for this drag, so the history reads as what was done.
    pub fn label(&self) -> &'static str {
        match self {
            LightGesture::Aim { .. } => "Aim Light",
            LightGesture::Move { .. } => "Move Light",
            LightGesture::Reach { .. } => "Light Reach",
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
pub fn target_at(scene: &Scene, point: Point, tolerance: f64) -> Option<LightGesture> {
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
        });
    }

    // A lamp's reach ring: grabbed anywhere on it.
    all.iter().find_map(|gizmo| match gizmo.kind {
        GizmoKind::Lamp { at, radius } => {
            let distance = (point - at).hypot();
            ((distance - radius).abs() <= tolerance)
                .then_some(LightGesture::Reach { light: gizmo.id })
        }
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
        let gesture = target_at(&scene, grabbed, GRAB_PX).expect("the lamp");
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
            target_at(&scene, on_ring, GRAB_PX),
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
            target_at(&scene, Point::new(-4000.0, -4000.0), GRAB_PX),
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
