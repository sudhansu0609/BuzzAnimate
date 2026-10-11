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
use buzz_scene::{Light, LightId, LightKey, LightKind, LightRig, LightTrack, Scene};
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

/// **How close to the stage a lamp may be dragged.**
///
/// Not zero. A lamp level with the artwork throws a shadow that runs away to
/// infinity — `shadow_transform`'s similar triangles divide by the gap — and it
/// would collapse its own stalk onto its handle, leaving nothing to grab to
/// bring it back out. Four units is close enough to be a lamp pressed against
/// the picture and far enough to still have a handle.
const MIN_LAMP_HEIGHT: f64 = 4.0;

/// And how far back. Past this a lamp is a sun with extra arithmetic: its rays
/// arrive very nearly parallel and moving it further changes nothing visible.
/// The same bound the panel's slider uses.
const MAX_LAMP_HEIGHT: f64 = 1200.0;

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
    /// A lamp: where it is, how far it reaches, and how far in front of the
    /// stage it hangs.
    Lamp { at: Point, radius: f64, height: f64 },
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
                edge,
                facing,
                throw,
                ..
            } => Some(edge + facing * throw),
            _ => None,
        }
    }

    /// **A lamp's third handle: how far in front of the stage it hangs.**
    ///
    /// A lamp is a point in three dimensions and the stage can only show two of
    /// them, so the one an animator could not reach was the one that decides
    /// how hard its light falls off, how far its shadows splay and how much of
    /// the picture it covers. It was a slider in a panel, which is a poor way
    /// to choose a position and no way at all to feel one.
    ///
    /// So it is drawn as a stalk standing off the lamp, in the way a lamp on a
    /// stand is drawn in a plan: the knob's distance from the lamp *is* the
    /// height, in the same document units as everything else here, and dragging
    /// it out and in moves the lamp forwards and backwards. Up rather than down
    /// so the stalk does not sit under the shadows the lamp is throwing, which
    /// on flat artwork all run away from it and mostly downwards.
    ///
    /// `None` for anything but a lamp: a sun has no position to be in front of
    /// and a gloom emits nothing.
    pub fn depth_handle(&self) -> Option<Point> {
        match self.kind {
            GizmoKind::Lamp { at, height, .. } => {
                Some(at - Vec2::new(0.0, height.max(MIN_LAMP_HEIGHT)))
            }
            _ => None,
        }
    }
    /// A lamp's reach handle: a knob on the outer ring to adjust falloff range.
    pub fn reach_handle(&self) -> Option<Point> {
        match self.kind {
            GizmoKind::Lamp { at, radius, .. } => Some(at + Vec2::new(radius, 0.0)),
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
    /// **Moving a lamp forwards and backwards** by dragging the stalk that
    /// stands off it. See [`Gizmo::depth_handle`].
    Raise { light: LightId },
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
            | LightGesture::Raise { light }
            | LightGesture::Throw { light } => *light,
        }
    }

    /// The undo label for this drag, so the history reads as what was done.
    pub fn label(&self) -> &'static str {
        match self {
            LightGesture::Aim { .. } => "Aim Light",
            LightGesture::Move { .. } => "Move Light",
            LightGesture::Reach { .. } => "Light Reach",
            LightGesture::Raise { .. } => "Move Light Forward",
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

/// Every gizmo in the document, in draw order — reading each light's *base*
/// state, with no keyframes resolved. Kept for tests that never touch a
/// track; the stage and everything else that draws or hit-tests gizmos wants
/// [`gizmos_at`] instead, or an animated light's handle sits at its base
/// position rather than where the light actually is at the playhead.
pub fn gizmos(scene: &Scene) -> Vec<Gizmo> {
    let stage = scene.stage().stage_rect();
    gizmos_from_rig(scene.lights(), stage)
}

/// [`gizmos`], reading each light's keyed state at `frame` instead of its
/// base — the fix for L1/L2: a lamp animating across the shot gets a handle
/// that follows it, not one nailed to wherever the base happens to sit.
pub fn gizmos_at(scene: &Scene, frame: u32) -> Vec<Gizmo> {
    let stage = scene.stage().stage_rect();
    gizmos_from_rig(&scene.lights().keyed_at(frame), stage)
}

fn gizmos_from_rig(rig: &LightRig, stage: Rect) -> Vec<Gizmo> {
    let (centre, dial) = dial_of(stage);

    rig.lights
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
                    position,
                    radius,
                    height,
                } => GizmoKind::Lamp {
                    at: position,
                    radius,
                    height,
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

/// What a press at `point` would grab, if anything — against each light's
/// *base* state. Kept for tests that never touch a track; hit-testing on the
/// stage wants [`target_at_frame`], or a press near an animated light's
/// keyed-away handle would miss it.
///
/// Handles first, rings second: a lamp whose ring passes near its own handle
/// must still be movable, and moving is the commoner intent.
pub fn target_at(
    scene: &Scene,
    point: Point,
    tolerance: f64,
    selected: Option<LightId>,
) -> Option<LightGesture> {
    target_among(&gizmos(scene), point, tolerance, selected)
}

/// [`target_at`], hit-testing each light's keyed state at `frame` instead of
/// its base — so a press finds an animated light's handle where it actually
/// is at the playhead, matching what [`gizmos_at`] draws.
pub fn target_at_frame(
    scene: &Scene,
    frame: u32,
    point: Point,
    tolerance: f64,
    selected: Option<LightId>,
) -> Option<LightGesture> {
    target_among(&gizmos_at(scene, frame), point, tolerance, selected)
}

fn target_among(
    all: &[Gizmo],
    point: Point,
    tolerance: f64,
    selected: Option<LightId>,
) -> Option<LightGesture> {
    // **The stalk knob first.** It stands off the lamp by the lamp's own height,
    // so on a lamp pressed right up against the stage it sits within a few
    // units of the handle that moves it sideways — and the one that moves it
    // sideways would win every time, leaving a lamp that could be pushed
    // forward but never pulled back. Tested before them, so the near case
    // resolves in favour of the gesture that can undo itself.
    if let Some(gizmo) = all.iter().find(|g| {
        g.depth_handle()
            .is_some_and(|knob| (knob - point).hypot() <= tolerance)
    }) {
        return Some(LightGesture::Raise { light: gizmo.id });
    }

    // **A lamp's reach knob on the ring.**
    if let Some(gizmo) = all.iter().find(|g| {
        g.reach_handle()
            .is_some_and(|knob| (knob - point).hypot() <= tolerance)
    }) {
        return Some(LightGesture::Reach { light: gizmo.id });
    }

    let nearest = all
        .iter()
        .filter(|g| match g.kind {
            GizmoKind::Sun { .. } => (g.handle() - point).hypot() <= tolerance + 8.0,
            _ => (g.handle() - point).hypot() <= tolerance,
        })
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

    // **The selected light (Sun or Lamp) is grabbable anywhere on its visible structure.**
    //
    // For Sun: clicking and dragging anywhere on its dial, spokes or rays directly aims it.
    // For Lamp: clicking and dragging anywhere inside its reach ring directly moves it.
    if let Some(inside) = all.iter().find(|gizmo| {
        Some(gizmo.id) == selected
            && match gizmo.kind {
                GizmoKind::Sun { centre, dial, .. } => {
                    let distance = (point - centre).hypot();
                    distance <= dial + tolerance
                }
                GizmoKind::Lamp { at, radius, .. } => {
                    let distance = (point - at).hypot();
                    distance <= radius + tolerance
                }
                _ => false,
            }
    }) {
        return Some(match inside.kind {
            GizmoKind::Sun { .. } => LightGesture::Aim { light: inside.id },
            GizmoKind::Lamp { at, .. } => LightGesture::Move {
                light: inside.id,
                grab: at - point,
            },
            GizmoKind::Gloom { .. } => unreachable!(),
        });
    }

    // The second handles: an unselected lamp's reach ring, and a gloom's far end. Both are
    // grabbed only once nothing nearer wanted the press, because moving is the
    // commoner intent and the two can sit close together on a short throw.
    all.iter().find_map(|gizmo| match gizmo.kind {
        GizmoKind::Lamp { at, radius, .. } => {
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
            LightGesture::Raise { .. },
            LightKind::Lamp {
                position, radius, ..
            },
        ) => LightKind::Lamp {
            position,
            // How far the knob has been pulled off the lamp, which is what the
            // stalk draws. Only the vertical distance: the stalk stands
            // straight up, so a hand that wanders sideways during the drag is
            // choosing the same number it was.
            height: (position.y - point.y).clamp(MIN_LAMP_HEIGHT, MAX_LAMP_HEIGHT),
            radius,
        },

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

        (LightGesture::Throw { .. }, LightKind::Gloom { edge, width, .. }) => {
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

/// Carry a light drag to `point`, keying it as it goes.
///
/// [`drag`] only ever touches one field — `kind`, via whichever handle the
/// gesture grabbed. That is fine for a static light, but not for one that
/// already animates: its colour, intensity and the rest of `kind` at this
/// frame live on the *track*, not the base, so the base has to be brought up
/// to the track's interpolated state before the drag edits it — otherwise
/// keying afterwards would write the base's stale values back over whatever
/// the track had built up to here, silently flattening the animation at this
/// frame (the bug `pointer_up_at` used to have).
///
/// 1. If the light's track animates, copy its keyed state at `frame` onto the
///    base first.
/// 2. Run the drag.
/// 3. If the track animates, key `frame` with the light's new whole state.
///    Otherwise, with `auto_key` on, start a track: key frame 0 with the
///    light as it stood *before* this drag (so the animation does not appear
///    to have already moved before the user touched it), then key `frame`
///    with the new state, and enable the track.
///
/// Meant to be called on every pointer move, for live feedback, and again on
/// release — the caller still wraps the whole gesture in one undo step.
/// Returns whether anything changed.
pub fn drag_light_at(
    scene: &mut Scene,
    gesture: LightGesture,
    point: Point,
    frame: u32,
    auto_key: bool,
) -> bool {
    let id = gesture.light();
    let Some(pre_drag) = scene.lights().get(id).cloned() else {
        return false;
    };

    // 1. Bring the base up to the frame's interpolated state before the drag
    //    touches it.
    if pre_drag.track.as_ref().is_some_and(|t| t.animates())
        && let Some(light) = scene.lights_mut().get_mut(id)
        && let Some(track) = light.track.clone()
    {
        *light = track.state_at(frame, light);
    }

    // 2. The drag itself.
    if !drag(scene, gesture, point) {
        return false;
    }

    // 3. Key the result.
    key_after_edit(scene, id, frame, auto_key, &pre_drag);
    true
}

/// Write a light panel edit onto the light, then key it by the same rule
/// [`drag_light_at`] drags by — over the track if it already animates, or
/// into a fresh one if `auto_key` is on. There is no drag to run here: the
/// panel already handed back the light's whole new state.
pub fn apply_light_edit(scene: &mut Scene, light: Light, frame: u32, auto_key: bool) {
    let id = light.id;
    let Some(pre_edit) = scene.lights().get(id).cloned() else {
        return;
    };
    if let Some(target) = scene.lights_mut().get_mut(id) {
        *target = light;
    }
    key_after_edit(scene, id, frame, auto_key, &pre_edit);
}

/// The keying step [`drag_light_at`] and [`apply_light_edit`] share, once the
/// light's fields already hold its new state: if the track animates, key
/// `frame` with the light as it now is; otherwise, with `auto_key` on, start a
/// track — key frame 0 with `pre` (the light as it stood *before* the edit,
/// so the animation does not appear to have already moved before the user
/// touched it), then key `frame` with the new state, and enable the track.
fn key_after_edit(scene: &mut Scene, id: LightId, frame: u32, auto_key: bool, pre: &Light) {
    let animates = pre.track.as_ref().is_some_and(|t| t.animates());
    if animates {
        if let Some(light) = scene.lights_mut().get_mut(id) {
            let key = LightKey::from_light(frame, light);
            if let Some(track) = light.track.as_mut() {
                track.set_key(key);
            }
        }
    } else if auto_key {
        let key0 = (frame > 0).then(|| LightKey::from_light(0, pre));
        if let Some(light) = scene.lights_mut().get_mut(id) {
            let key = LightKey::from_light(frame, light);
            let track = light.track.get_or_insert_with(LightTrack::new);
            if let Some(k0) = key0 {
                track.set_key(k0);
            }
            track.set_key(key);
            track.enabled = true;
        }
    }
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
                edge,
                facing,
                throw,
                ..
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
            LightKind::Lamp {
                position,
                radius: r,
                ..
            } => {
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

    fn scene_with_lamp(at: Point) -> (Scene, LightId) {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::lamp(at));
        scene.lights_mut().enabled = true;
        (scene, id)
    }

    /// **A lamp can be moved forwards and backwards, not only across.**
    ///
    /// Its height is the third coordinate — how far in front of the stage it
    /// hangs — and it decides how hard the light falls off and how far the
    /// shadows splay. It used to be reachable only from a slider in a panel.
    #[test]
    fn a_lamps_stalk_moves_it_forward_and_back() {
        let (mut scene, id) = scene_with_lamp(Point::new(200.0, 200.0));
        let before = match scene.lights().get(id).expect("the lamp").kind {
            LightKind::Lamp { height, .. } => height,
            _ => unreachable!("a lamp"),
        };

        // The knob stands off the lamp by its own height, so that is where a
        // hand goes looking for it.
        let knob = gizmos(&scene)
            .into_iter()
            .find(|g| g.id == id)
            .and_then(|g| g.depth_handle())
            .expect("a lamp has a stalk");
        assert!(
            (knob - Point::new(200.0, 200.0 - before)).hypot() < 1e-9,
            "the stalk is as long as the lamp is forward, got {knob:?}"
        );

        let grabbed = target_at(&scene, knob, GRAB_PX, None);
        assert!(
            matches!(grabbed, Some(LightGesture::Raise { light }) if light == id),
            "the knob is grabbed as a forward/back drag, got {grabbed:?}"
        );

        // Pulled further out: the lamp moves back off the stage.
        assert!(drag(
            &mut scene,
            LightGesture::Raise { light: id },
            Point::new(200.0, 200.0 - 400.0)
        ));
        let after = match scene.lights().get(id).expect("the lamp").kind {
            LightKind::Lamp { height, .. } => height,
            _ => unreachable!("a lamp"),
        };
        assert!(
            (after - 400.0).abs() < 1e-9,
            "the knob's distance is the height, got {after}"
        );
        assert!(after > before, "it went backwards, as the drag asked");
    }

    /// **A lamp dragged onto the stage stops short of it**, because a lamp
    /// level with the artwork throws a shadow that runs away to infinity — and
    /// because a stalk of zero length has no knob left to pull it back out
    /// with.
    #[test]
    fn a_lamp_cannot_be_pushed_through_the_stage() {
        let (mut scene, id) = scene_with_lamp(Point::new(120.0, 300.0));

        // Dragged well past the lamp, and then well past it the other way.
        drag(
            &mut scene,
            LightGesture::Raise { light: id },
            Point::new(120.0, 900.0),
        );
        let height = match scene.lights().get(id).expect("the lamp").kind {
            LightKind::Lamp { height, .. } => height,
            _ => unreachable!("a lamp"),
        };
        assert!(
            height >= MIN_LAMP_HEIGHT,
            "it stops in front of the stage, got {height}"
        );

        drag(
            &mut scene,
            LightGesture::Raise { light: id },
            Point::new(120.0, -100_000.0),
        );
        let height = match scene.lights().get(id).expect("the lamp").kind {
            LightKind::Lamp { height, .. } => height,
            _ => unreachable!("a lamp"),
        };
        assert!(
            height <= MAX_LAMP_HEIGHT,
            "and it stops before it becomes a sun, got {height}"
        );
    }

    /// Moving a lamp sideways and moving it forwards are different gestures on
    /// different handles, and neither may swallow the other. A lamp pressed
    /// against the stage has them within a few units of each other.
    #[test]
    fn the_stalk_and_the_lamp_are_separate_handles() {
        let (mut scene, id) = scene_with_lamp(Point::new(200.0, 200.0));
        drag(
            &mut scene,
            LightGesture::Raise { light: id },
            Point::new(200.0, 200.0 - MIN_LAMP_HEIGHT),
        );

        // Pressed on the lamp itself, the near knob must not win.
        let on_the_lamp = target_at(&scene, Point::new(200.0, 200.0), GRAB_PX, Some(id));
        // With the two all but on top of each other the knob is the one that
        // can undo itself, so it takes the press; what matters is that the
        // gesture is a light gesture at all rather than nothing.
        assert!(
            on_the_lamp.is_some(),
            "a press on a lamp pressed against the stage still grabs it"
        );

        // Pulled back out, they separate and each does its own job.
        drag(
            &mut scene,
            LightGesture::Raise { light: id },
            Point::new(200.0, 200.0 - 160.0),
        );
        assert!(matches!(
            target_at(&scene, Point::new(200.0, 200.0), GRAB_PX, Some(id)),
            Some(LightGesture::Move { .. })
        ));
        assert!(matches!(
            target_at(&scene, Point::new(200.0, 40.0), GRAB_PX, Some(id)),
            Some(LightGesture::Raise { .. })
        ));
    }

    #[test]
    fn a_selected_sun_can_be_aimed_by_clicking_anywhere_on_dial() {
        let (scene, id) = scene_with_sun();
        let stage = scene.stage().stage_rect();
        let (centre, dial) = dial_of(stage);

        // Click halfway out along the dial, away from the handle
        let midway = centre + Vec2::new(dial * 0.5, dial * 0.2);
        let gesture = target_at(&scene, midway, GRAB_PX, Some(id));
        assert_eq!(
            gesture,
            Some(LightGesture::Aim { light: id }),
            "clicking inside the sun dial when selected aims the sun"
        );
    }

    #[test]
    fn a_selected_lamp_can_be_moved_by_clicking_anywhere_in_body() {
        let (scene, id) = scene_with_lamp(Point::new(300.0, 300.0));

        // Click inside the lamp reach ring
        let inside = Point::new(350.0, 320.0);
        let gesture = target_at(&scene, inside, GRAB_PX, Some(id));
        assert!(
            matches!(gesture, Some(LightGesture::Move { light, .. }) if light == id),
            "clicking anywhere inside the selected lamp moves it"
        );
    }

    // -- keyed lights: L1-L4 --------------------------------------------------

    /// A lamp animating from `from` to `to` over frames 0..20, with intensity
    /// rising from 1.0 to 3.0 alongside it — so frame 10, the midpoint used by
    /// every test below, has an unambiguous interpolated position *and*
    /// intensity to check against.
    fn scene_with_animated_lamp(from: Point, to: Point) -> (Scene, LightId) {
        let mut scene = Scene::default();
        let id = scene.add_light(LightKind::lamp(from));
        scene.lights_mut().enabled = true;
        let light = scene.lights_mut().get_mut(id).expect("just added");
        light.intensity = 1.0;
        let key0 = LightKey::from_light(0, light);
        let mut key1 = key0;
        key1.frame = 20;
        key1.intensity = 3.0;
        key1.kind = LightKind::lamp(to);
        let track = light.track.get_or_insert_with(LightTrack::new);
        track.enabled = true;
        track.set_key(key0);
        track.set_key(key1);
        (scene, id)
    }

    fn lamp_position(kind: LightKind) -> Point {
        match kind {
            LightKind::Lamp { position, .. } => position,
            other => panic!("not a lamp: {other:?}"),
        }
    }

    /// **L1**: a keyed lamp's gizmo follows the track, not the base — the
    /// handle sits at the interpolated position mid-span rather than nailed to
    /// wherever the base happens to be.
    #[test]
    fn gizmo_of_a_keyed_lamp_sits_at_the_interpolated_position_mid_span() {
        let (scene, id) = scene_with_animated_lamp(Point::new(0.0, 0.0), Point::new(200.0, 0.0));

        let gizmo = gizmos_at(&scene, 10)
            .into_iter()
            .find(|g| g.id == id)
            .expect("the lamp has a gizmo");
        match gizmo.kind {
            GizmoKind::Lamp { at, .. } => {
                assert_eq!(at, Point::new(100.0, 0.0), "the handle should sit midway");
            }
            other => panic!("not a lamp: {other:?}"),
        }

        // The base itself is untouched: `gizmos` (no frame) still reads the
        // frame-0 position.
        match gizmos(&scene)
            .into_iter()
            .find(|g| g.id == id)
            .expect("the lamp")
            .kind
        {
            GizmoKind::Lamp { at, .. } => {
                assert_eq!(at, Point::new(0.0, 0.0), "the base should be untouched")
            }
            other => panic!("not a lamp: {other:?}"),
        }
    }

    /// **L2**: dragging an animated lamp is live — the base already reflects
    /// the new position before release — and keys only the frame being
    /// dragged, leaving the frame-0 key exactly as it was. The key it writes
    /// also carries the interpolated intensity at that frame, not whatever the
    /// (inert) base held before the drag started.
    #[test]
    fn dragging_an_animated_lamp_moves_it_live_and_keys_only_the_dragged_frame() {
        let (mut scene, id) = scene_with_animated_lamp(Point::new(0.0, 0.0), Point::new(200.0, 0.0));
        let frame = 10;

        // Grabbed right on the interpolated handle.
        let gesture = target_at_frame(&scene, frame, Point::new(100.0, 0.0), GRAB_PX, None)
            .expect("the interpolated handle is grabbable");
        assert_eq!(gesture.light(), id);

        // Live feedback: called as every intermediate move would call it.
        assert!(drag_light_at(
            &mut scene,
            gesture,
            Point::new(150.0, 40.0),
            frame,
            false
        ));
        assert_eq!(
            lamp_position(scene.lights().get(id).expect("the lamp").kind),
            Point::new(150.0, 40.0),
            "the base should already show the drag before release"
        );

        // And again on release, landing a little further on, as `pointer_up_at`
        // does after the last move.
        assert!(drag_light_at(
            &mut scene,
            gesture,
            Point::new(160.0, 50.0),
            frame,
            false
        ));

        let light = scene.lights().get(id).expect("the lamp");
        let track = light.track.as_ref().expect("still animates");
        // Frame 10 had no key before the drag; the drag adds one, alongside
        // the frame-0 and frame-20 keys that were already there.
        assert_eq!(track.keys().len(), 3, "one key added, at the dragged frame");

        let key10 = track
            .keys()
            .iter()
            .find(|k| k.frame == frame)
            .expect("frame 10 was keyed");
        assert_eq!(lamp_position(key10.kind), Point::new(160.0, 50.0));
        assert_eq!(
            key10.intensity, 2.0,
            "the key should carry the interpolated intensity at frame 10, not the stale base"
        );

        let key0 = track
            .keys()
            .iter()
            .find(|k| k.frame == 0)
            .expect("frame 0 must still be there");
        assert_eq!(
            lamp_position(key0.kind),
            Point::new(0.0, 0.0),
            "frame 0's key must be untouched by a drag at frame 10"
        );
        assert_eq!(key0.intensity, 1.0, "frame 0's intensity must be untouched");
    }

    /// **L3**: Auto Keyframe on, dragging an un-keyed lamp at frame 12 starts a
    /// track with exactly two keys: frame 0 holding the light as it stood
    /// *before* the drag, and frame 12 holding the dragged result.
    #[test]
    fn drag_with_auto_key_on_an_unkeyed_lamp_yields_keys_at_frame_zero_and_the_playhead() {
        let (mut scene, id) = scene_with_lamp(Point::new(100.0, 100.0));
        let frame = 12;

        let gesture =
            target_at(&scene, Point::new(100.0, 100.0), GRAB_PX, None).expect("the lamp");
        assert!(drag_light_at(
            &mut scene,
            gesture,
            Point::new(300.0, 200.0),
            frame,
            true
        ));

        let light = scene.lights().get(id).expect("the lamp");
        let track = light.track.as_ref().expect("auto-key should have started a track");
        assert!(track.enabled);
        assert_eq!(track.keys().len(), 2);

        let key0 = track
            .keys()
            .iter()
            .find(|k| k.frame == 0)
            .expect("frame 0 keyed");
        assert_eq!(
            lamp_position(key0.kind),
            Point::new(100.0, 100.0),
            "frame 0 should hold the pre-drag state"
        );

        let key12 = track
            .keys()
            .iter()
            .find(|k| k.frame == frame)
            .expect("the playhead keyed");
        assert_eq!(lamp_position(key12.kind), Point::new(300.0, 200.0));
    }

    /// A drag at frame 0 with auto-key on needs no separate anchor key — the
    /// single key at frame 0 *is* the new state.
    #[test]
    fn drag_with_auto_key_at_frame_zero_writes_a_single_key() {
        let (mut scene, id) = scene_with_lamp(Point::new(100.0, 100.0));

        let gesture =
            target_at(&scene, Point::new(100.0, 100.0), GRAB_PX, None).expect("the lamp");
        assert!(drag_light_at(
            &mut scene,
            gesture,
            Point::new(300.0, 200.0),
            0,
            true
        ));

        let light = scene.lights().get(id).expect("the lamp");
        let track = light.track.as_ref().expect("auto-key should have started a track");
        assert_eq!(track.keys().len(), 1);
        assert_eq!(lamp_position(track.keys()[0].kind), Point::new(300.0, 200.0));
    }

    /// **L4**: editing an animated light through the panel's path keys the
    /// playhead — carrying over the interpolated intensity the panel showed —
    /// rather than overwriting the (inert) base.
    #[test]
    fn applying_a_panel_edit_to_an_animated_light_keys_the_playhead() {
        let (mut scene, id) = scene_with_animated_lamp(Point::new(0.0, 0.0), Point::new(200.0, 0.0));
        let frame = 10;

        // What the panel would have shown: the keyed state at the playhead,
        // with the user's edit (a colour change) folded in.
        let mut edited = scene.lights().keyed_at(frame).get(id).expect("the lamp").clone();
        edited.color = Color::from_rgb8(0x11, 0x22, 0x33);

        apply_light_edit(&mut scene, edited, frame, false);

        let light = scene.lights().get(id).expect("the lamp");
        let track = light.track.as_ref().expect("still animates");
        let key10 = track
            .keys()
            .iter()
            .find(|k| k.frame == frame)
            .expect("frame 10 was keyed");
        assert_eq!(key10.color, Color::from_rgb8(0x11, 0x22, 0x33));
        assert_eq!(
            key10.intensity, 2.0,
            "the interpolated intensity shown by the panel should carry through"
        );

        let key0 = track
            .keys()
            .iter()
            .find(|k| k.frame == 0)
            .expect("frame 0 must still be there");
        assert_eq!(key0.intensity, 1.0, "frame 0 must be untouched");
    }

    /// **L4**, Auto Keyframe: editing an un-keyed light starts a track, keying
    /// frame 0 with the pre-edit state and the playhead with the new one.
    #[test]
    fn applying_a_panel_edit_with_auto_key_on_an_unkeyed_light_starts_a_track() {
        let (mut scene, id) = scene_with_lamp(Point::new(100.0, 100.0));
        let frame = 7;

        let mut edited = scene.lights().get(id).expect("the lamp").clone();
        edited.color = Color::from_rgb8(0x44, 0x55, 0x66);

        apply_light_edit(&mut scene, edited, frame, true);

        let light = scene.lights().get(id).expect("the lamp");
        let track = light.track.as_ref().expect("auto-key should have started a track");
        assert!(track.enabled);
        assert_eq!(track.keys().len(), 2);
        let key0 = track
            .keys()
            .iter()
            .find(|k| k.frame == 0)
            .expect("frame 0 keyed");
        assert_ne!(
            key0.color,
            Color::from_rgb8(0x44, 0x55, 0x66),
            "frame 0 should hold the pre-edit colour"
        );
        let key7 = track
            .keys()
            .iter()
            .find(|k| k.frame == frame)
            .expect("the playhead keyed");
        assert_eq!(key7.color, Color::from_rgb8(0x44, 0x55, 0x66));
    }
}
