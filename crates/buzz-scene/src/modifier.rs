//! Live modifiers: procedural motion evaluated at draw time.
//!
//! # What a modifier is
//!
//! A [`Modifier`] on an [`crate::Object`] is a small rule that changes how the
//! object is drawn at each frame — a spring that makes hair follow through, a
//! wiggle that keeps a held pose breathing. Unlike a baked performance it is
//! **not written to keyframes**: it is evaluated when the frame is drawn and is
//! deterministic in `(object id, frame)`, so the result is reproducible and,
//! crucially, stays in sync when the underlying animation is re-timed. Re-time
//! the walk and the sprung tail re-follows it, with nothing to re-bake.
//!
//! # Why the same maths as the bakers
//!
//! The spring and the wiggle here are the *same* solvers the bakers use
//! ([`buzz_physics`], [`buzz_rig::follow_through`]). "Live" and "bake to
//! keyframes" are two deliveries of one calculation: the baker writes it down
//! once; the modifier runs it every draw. Keeping one solver behind both is why
//! a live spring and a baked one produce the identical motion.
//!
//! The data lives here; the evaluation (and its cache, since a spring must be
//! integrated forward across the whole span) lives on [`crate::Scene`].

/// One live effect on an object, evaluated at draw time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Modifier {
    /// A deterministic wandering offset on the object's placement — idle sway,
    /// a breeze, a handheld shake. Stateless: the offset at a frame depends only
    /// on the object and the frame.
    Wiggle { amplitude: f64, frequency: f64 },
    /// Damped-spring follow-through on the bone chain rooted at `root` (that bone
    /// and everything below it), driven by the object's keyed motion and — when
    /// `coupling` is above zero — by the whole body's movement. Rigs only.
    Spring {
        root: usize,
        stiffness: f64,
        damping: f64,
        coupling: f64,
    },
    /// Turn the object to face a point in stage space — eyes and heads that
    /// track a target. Its own +x axis is aimed at `(x, y)`.
    LookAt { x: f64, y: f64 },
    /// Stretch the object along its direction of motion and squash it across,
    /// by `amount` per unit of speed. Volume-preserving: a fast move thins and
    /// lengthens the drawing, the oldest trick for selling weight and speed.
    AutoSquashStretch { amount: f64 },
    /// **Breathing.** The chest rises and falls: the drawing grows a little
    /// taller and wider about its own feet, in and out, forever.
    ///
    /// # Why a character needs one
    ///
    /// A held pose in animation is never *still*. A drawing that does not move
    /// between two keys reads as a picture of a character rather than as a
    /// character standing there, and the cheapest thing that fixes it — the
    /// thing every animator draws by hand on a hold — is a breath. It is two
    /// per cent of scale and nobody ever notices it consciously; they notice
    /// its absence immediately.
    ///
    /// `rate` is in **breaths per minute** — twelve to sixteen at rest, thirty
    /// and up after running — and `depth` scales the whole thing, `1.0` being
    /// a comfortable resting breath.
    ///
    /// Anchored at the bottom of the drawing, so the feet stay on the ground
    /// and the motion goes into the chest, which is where a breath belongs.
    /// The phase is seeded from the object's id, so a crowd does not breathe
    /// in unison — which is the one thing that would make it visible.
    Breathe { rate: f64, depth: f64 },
    /// **Wind.** The drawing bends downwind from its base, in gusts.
    ///
    /// A shear rather than a rotation: the bottom stays planted and the lean
    /// grows with height, which is what a trunk does and what a rotation does
    /// not — a rotated tree pivots its roots out of the ground.
    ///
    /// `amount` is how far the top leans at a full gust, as a fraction of the
    /// drawing's own height (`0.1` is a stiff pine, `0.35` a willow); `rate` is
    /// the gust frequency in hertz, around `0.2` for a breeze.
    ///
    /// The gust is **biased downwind** rather than centred, because wind is:
    /// it lulls back towards upright and gusts one way, instead of waving the
    /// tree evenly to both sides like a metronome. Seeded from the object's id,
    /// so a row of trees planted from the same drawing does not sway as one
    /// object — which is exactly what gives a painted background away.
    Sway { amount: f64, rate: f64 },
    /// **A steady drift, wrapping.** The object travels at `(dx, dy)` document
    /// units per second, and every `span` units of travel it is back where it
    /// started.
    ///
    /// # What it is for
    ///
    /// Everything in a background that goes past rather than moves about:
    /// clouds crossing the sky, the surface of a river, a streetscape behind a
    /// window, snow across a shot. All of it is one velocity and a loop, and
    /// all of it used to be two keyframes per object per shot — which is fine
    /// until the shot is re-timed, and then it is wrong everywhere at once.
    ///
    /// # Why the wrap is a distance and not a rectangle
    ///
    /// A wrap needs to know how far to go before starting again, and the honest
    /// answer depends on the *drawing*: a cloud has to be all the way off the
    /// stage before it can come back on, or it pops in mid-frame. That is a
    /// number the thing placing the cloud knows and the modifier does not, so
    /// it is passed in. `span` of zero never wraps, which is what a one-way
    /// move across a single shot wants.
    ///
    /// The distance is measured **along the drift**, not per axis, so a
    /// diagonal drift loops once rather than beating between two periods.
    ///
    /// `phase` is how far into that loop the object already is, `0..1`, and it
    /// is the field that makes a *field* of drifting things possible. Without
    /// it five clouds on one loop are five clouds in a queue: they all start at
    /// the left edge together and cross in formation. Offsetting where each one
    /// is *placed* does not fix it — the wrap then sends the ones placed
    /// further along off the far side and holds them there for most of the
    /// loop, which is exactly what it looked like. The phase has to be inside
    /// the modulo, so it lives here.
    Drift {
        dx: f64,
        dy: f64,
        span: f64,
        phase: f64,
    },
}

impl Modifier {
    /// A short name for the status line and menus.
    pub fn label(&self) -> &'static str {
        match self {
            Modifier::Wiggle { .. } => "Wiggle",
            Modifier::Spring { .. } => "Spring",
            Modifier::LookAt { .. } => "Look At",
            Modifier::AutoSquashStretch { .. } => "Squash & Stretch",
            Modifier::Breathe { .. } => "Breathe",
            Modifier::Sway { .. } => "Sway",
            Modifier::Drift { .. } => "Drift",
        }
    }

    /// Does this modifier change the object's pose/geometry (and so needs an
    /// owned, re-posed copy), rather than only prepending a transform?
    pub fn changes_pose(&self) -> bool {
        matches!(self, Modifier::Spring { .. })
    }
}

use std::collections::HashMap;
use std::sync::Arc;

use buzz_geom::Affine;
use buzz_physics::{Spring, Wiggle, wiggle_at};

use crate::{LayerId, Object, ObjectId, ObjectKind, Scene};

/// **One breath, in `-1..=1`.**
///
/// Not a sine. A breath is not symmetric: the chest fills quickly and empties
/// slowly, and a pure sine reads as a machine — which is the difference
/// between a character breathing and a drawing pulsing. A second harmonic at a
/// third of the amplitude sharpens the rise and lengthens the fall, which is
/// the shape of the real thing and costs one more `sin`.
///
/// `rate` is in breaths per minute; `seed` is the object's id, and only moves
/// the phase, so a crowd breathes at the same rate without breathing together.
fn breath_at(seed: u64, rate: f64, t_seconds: f64) -> f64 {
    use std::f64::consts::TAU;
    let per_second = rate.clamp(0.5, 120.0) / 60.0;
    // A stable phase per object, from the same hash the wiggle uses.
    let phase = ((splitmix64(seed ^ 0xB2EA_7115) as f64) / u64::MAX as f64) * TAU;
    let a = TAU * per_second * t_seconds + phase;
    (a.sin() + 0.33 * (2.0 * a).sin()) / 1.33
}

/// **One gust of wind, in about `-0.3..=1.0`.**
///
/// Biased downwind, because wind is: it lulls back towards upright and pushes
/// one way, rather than waving a tree evenly to both sides. The wander itself
/// is the wiggle's own fractal sum of sines — three octaves, so the branch has
/// a flutter on top of the gust rather than a single frequency, which is what
/// stops a row of trees looking like windscreen wipers.
fn gust_at(seed: u64, rate: f64, t_seconds: f64) -> f64 {
    let wander = buzz_physics::wiggle_at(
        buzz_physics::Wiggle {
            amplitude: 1.0,
            frequency: rate.clamp(0.01, 20.0),
        },
        seed ^ 0x5EED_1A15,
        t_seconds,
    );
    0.35 + 0.65 * wander.dx
}

/// SplitMix64's finalizer, for a stable phase per object. The same mixer
/// `buzz_physics` seeds its wiggles with, so two procedural motions on one
/// object do not share a phase by accident.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The spring cache's table: a full modified pose sequence per `(object, chain
/// root)`, all built for one document revision.
pub(crate) type SpringTable = HashMap<(ObjectId, usize), Arc<Vec<Vec<f64>>>>;

/// The result of evaluating an object's modifiers at a frame.
///
/// `prepend` is composed onto the object's own transform in stage space (a
/// wiggle offset); `object` is a re-posed owned copy when a pose modifier (a
/// spring) changed the geometry, or `None` when only the transform moved and the
/// original — with its stable `Arc` identity — can be drawn.
#[derive(Debug, Clone)]
pub struct ModifierEval {
    pub prepend: Affine,
    pub object: Option<Object>,
}

impl Scene {
    /// Evaluate an object's live modifiers at `frame`.
    ///
    /// Returns `None` for the overwhelming majority of objects, which have no
    /// modifiers — the render path then draws them exactly as before. Otherwise
    /// returns the transform to prepend and, if a spring re-posed the rig, the
    /// modified object to draw in place of the original.
    pub fn modified_object_at(
        &self,
        layer: LayerId,
        object: &Object,
        at: impl crate::time::AtTime,
    ) -> Option<ModifierEval> {
        if object.modifiers.is_empty() {
            return None;
        }
        // A wiggle is a function of *time* and shakes fast enough to smear
        // within one frame, so it is sampled continuously. A spring's pose and
        // a squash's look-back are computed frame by frame and are held at the
        // frame the shutter opened on — see the note on each below.
        let time = at.as_time();
        let frame = at.frame();
        let fps = self.stage().frame_rate.max(1.0);
        let mut prepend = Affine::IDENTITY;
        let mut posed: Option<Object> = None;

        for modifier in &object.modifiers {
            match *modifier {
                Modifier::Wiggle {
                    amplitude,
                    frequency,
                } => {
                    let sample = wiggle_at(
                        Wiggle {
                            amplitude,
                            frequency,
                        },
                        object.id.0,
                        // Continuous: this is what lets a shake blur.
                        time / fps,
                    );
                    prepend = Affine::translate((sample.dx, sample.dy)) * prepend;
                }
                Modifier::Spring {
                    root,
                    stiffness,
                    damping,
                    coupling,
                } => {
                    // Integrated frame by frame, so there is no state between two of
                    // them to ask for: the pose is held across the shutter.
                    let spring = Spring { stiffness, damping };
                    if let Some(seq) = self.spring_sequence(layer, object, root, spring, coupling, fps)
                    {
                        let index = (frame as usize).min(seq.len().saturating_sub(1));
                        if let Some(pose) = seq.get(index) {
                            let target = posed.get_or_insert_with(|| object.clone());
                            if let ObjectKind::Armature(rig) = &mut target.kind {
                                rig.armature.set_pose(pose);
                            }
                        }
                    }
                }
                Modifier::LookAt { x, y } => {
                    // Turn the object about its own anchor so its +x axis points
                    // at the target, on top of whatever rotation it already has.
                    let base = object.transform;
                    let anchor = base.translation();
                    let coeffs = base.as_coeffs();
                    let base_angle = coeffs[1].atan2(coeffs[0]);
                    let desired = (y - anchor.y).atan2(x - anchor.x);
                    let turn = desired - base_angle;
                    prepend = Affine::translate(anchor)
                        * Affine::rotate(turn)
                        * Affine::translate(-anchor)
                        * prepend;
                }
                Modifier::AutoSquashStretch { amount } => {
                    // Speed from the previous frame's placement of this same
                    // object — one look-back, no integration.
                    let here = object.transform.translation();
                    let before = if frame == 0 {
                        here
                    } else {
                        self.layers()
                            .get(layer)
                            .and_then(|l| {
                                l.frames
                                    .resolved_at(frame - 1)
                                    .iter()
                                    .find(|o| o.id == object.id)
                                    .map(|o| o.transform.translation())
                            })
                            .unwrap_or(here)
                    };
                    let velocity = here - before;
                    let speed = velocity.hypot();
                    if speed > 1e-6 {
                        // Stretch along motion, squash across it; clamped so a
                        // teleport does not turn the drawing into a needle.
                        let stretch = (1.0 + amount * speed).clamp(0.25, 4.0);
                        let heading = velocity.y.atan2(velocity.x);
                        let squash = Affine::rotate(heading)
                            * Affine::scale_non_uniform(stretch, 1.0 / stretch)
                            * Affine::rotate(-heading);
                        prepend = Affine::translate(here)
                            * squash
                            * Affine::translate(-here)
                            * prepend;
                    }
                }
                Modifier::Breathe { rate, depth } => {
                    // Continuous in time, like the wiggle and for the same
                    // reason: a breath is slow, and sampling it per frame
                    // rather than per shutter would step it.
                    let bounds = object.bounds();
                    if bounds.width() > 0.0 && bounds.height() > 0.0 {
                        let s = breath_at(object.id.0, rate, time / fps);
                        let depth = depth.clamp(0.0, 4.0);
                        // **Two per cent, and taller than it is wider.** A
                        // breath you can measure is a breath the audience can
                        // see, and a character that visibly inflates reads as a
                        // balloon. The chest fills, so both axes grow; it fills
                        // upwards more than outwards, so y grows about twice as
                        // much as x.
                        let sy = 1.0 + depth * 0.022 * s;
                        let sx = 1.0 + depth * 0.010 * s;
                        // The feet, not the middle: a breath must not lift the
                        // character off the ground.
                        let feet = buzz_geom::Point::new(bounds.center().x, bounds.y1);
                        prepend = Affine::translate(feet.to_vec2())
                            * Affine::scale_non_uniform(sx, sy)
                            * Affine::translate(-feet.to_vec2())
                            * prepend;
                    }
                }
                Modifier::Drift {
                    dx,
                    dy,
                    span,
                    phase,
                } => {
                    // Continuous in time, like the wiggle: a drift is smooth,
                    // and sampling it per frame rather than per shutter would
                    // step a slow one visibly.
                    let seconds = time / fps;
                    let speed = (dx * dx + dy * dy).sqrt();
                    if speed > 1e-9 {
                        // Wrapped along the drift, so a background loops — and
                        // the head start goes *inside* the wrap, so a cloud
                        // that begins three quarters of the way along still
                        // spends the same share of its loop on screen as one
                        // that begins at the edge.
                        let travelled = if span > 1e-9 {
                            (speed * seconds + phase * span).rem_euclid(span)
                        } else {
                            speed * seconds
                        };
                        let step = travelled / speed;
                        prepend = Affine::translate((dx * step, dy * step)) * prepend;
                    }
                }
                Modifier::Sway { amount, rate } => {
                    let bounds = object.bounds();
                    if bounds.width() > 0.0 && bounds.height() > 0.0 {
                        let gust = gust_at(object.id.0, rate, time / fps);
                        // How far the *top* of the drawing leans, in document
                        // units: a fraction of its own height, so one setting
                        // suits a sapling and a full-grown oak.
                        let lean = amount.clamp(-2.0, 2.0) * gust * bounds.height();
                        // Shear: the displacement grows with height above the
                        // base, so the base itself does not move. `k` is that
                        // displacement per unit of height.
                        let k = lean / bounds.height();
                        let base = bounds.y1;
                        // A bend shortens what it bends — the top of a leaning
                        // trunk is nearer the ground than the top of an upright
                        // one. Without it the crown swings along an arc that is
                        // visibly wrong at the extremes, and the tree looks
                        // rubbery rather than woody.
                        let shorten = 1.0 / (1.0 + k * k).sqrt();
                        prepend = Affine::translate((0.0, base))
                            * Affine::new([1.0, 0.0, -k, shorten, 0.0, 0.0])
                            * Affine::translate((0.0, -base))
                            * prepend;
                    }
                }
            }
        }

        Some(ModifierEval {
            prepend,
            object: posed,
        })
    }

    /// The whole modified pose sequence for a spring on `object`'s chain, built
    /// once per document revision and cached. `None` if the object is not a rig
    /// or the chain root is out of range.
    fn spring_sequence(
        &self,
        layer: LayerId,
        object: &Object,
        root: usize,
        spring: Spring,
        coupling: f64,
        fps: f64,
    ) -> Option<Arc<Vec<Vec<f64>>>> {
        let key = (object.id, root);
        let revision = self.revision;

        // Hit: the cache is for this revision and already holds this chain.
        if let Ok(cache) = self.modifier_cache.read()
            && let Some((cached_revision, table)) = &*cache
            && *cached_revision == revision
            && let Some(seq) = table.get(&key)
        {
            return Some(seq.clone());
        }

        // Miss: reconstruct the primary motion off the timeline and integrate.
        let ObjectKind::Armature(data) = &object.kind else {
            return None;
        };
        let topology = data.armature.clone();
        if root >= topology.bones.len() {
            return None;
        }
        let layers = self.layers();
        let timeline = &layers.get(layer)?.frames;
        let span = timeline.length().max(1);

        let mut primary = Vec::with_capacity(span as usize);
        let mut world = Vec::with_capacity(span as usize);
        for g in 0..span {
            let resolved = timeline.resolved_at(g);
            let here = resolved.iter().find(|o| o.id == object.id);
            primary.push(
                here.and_then(|o| match &o.kind {
                    ObjectKind::Armature(r) => Some(r.armature.pose()),
                    _ => None,
                })
                .unwrap_or_else(|| topology.pose()),
            );
            world.push(here.map_or(Affine::IDENTITY, |o| o.transform));
        }

        let modified = if coupling > 0.0 {
            buzz_rig::follow_through_coupled(&topology, root, spring, &primary, &world, coupling, fps)
        } else {
            buzz_rig::follow_through(&topology, root, spring, &primary, fps)
        };
        let arc = Arc::new(modified);

        if let Ok(mut cache) = self.modifier_cache.write() {
            match &mut *cache {
                Some((cached_revision, table)) if *cached_revision == revision => {
                    table.insert(key, arc.clone());
                }
                // Stale or empty: start a fresh table for this revision.
                slot => {
                    let mut table = SpringTable::new();
                    table.insert(key, arc.clone());
                    *slot = Some((revision, table));
                }
            }
        }
        Some(arc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArmatureData, LayerKind, Object, ShapeData, Tween};
    use buzz_geom::{Affine, Point, Rect, Shape as _};
    use buzz_rig::{Armature, Bone};
    use peniko::Color;

    fn rig_scene() -> (Scene, LayerId, ObjectId) {
        let mut arm = Armature {
            root: Point::ORIGIN,
            bones: Vec::new(),
        };
        for i in 0..4 {
            let parent = if i == 0 { None } else { Some(i - 1) };
            arm.bones.push(Bone::new(format!("b{i}"), parent, 40.0, 0.0));
        }
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Rig", LayerKind::Normal);
        let object = Object {
            kind: ObjectKind::Armature(ArmatureData::new(arm)),
            ..Object::shape(
                ObjectId(1),
                ShapeData::filled(Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-9), Color::WHITE),
            )
        };
        let id = scene.add_object(layer, object).unwrap();
        (scene, layer, id)
    }

    /// Swing the base bone to 0.8 by frame 12, hold to `hold_to`.
    fn animate_base(scene: &mut Scene, layer: LayerId, id: ObjectId, hold_to: u32) {
        scene.update_layer(layer, |l| {
            if l.frames.length() <= 12 {
                l.frames.insert_frame(12);
            }
        });
        scene.ensure_keyframe(layer, 12);
        scene.update_object_at(12, id, |o| {
            if let ObjectKind::Armature(r) = &mut o.kind {
                r.armature.set_pose(&[0.8, 0.0, 0.0, 0.0]);
            }
        });
        scene.update_layer(layer, |l| {
            l.frames.set_tween(0, Tween::motion());
            if l.frames.length() <= hold_to {
                l.frames.insert_frame(hold_to);
            }
        });
    }

    fn resolved(scene: &Scene, layer: LayerId, id: ObjectId, frame: u32) -> Object {
        scene
            .layers()
            .get(layer)
            .unwrap()
            .frames
            .resolved_at(frame)
            .iter()
            .find(|o| o.id == id)
            .cloned()
            .unwrap_or_else(|| panic!("no object at frame {frame}"))
    }

    #[test]
    fn an_object_without_modifiers_evaluates_to_nothing() {
        let (scene, layer, id) = rig_scene();
        let obj = resolved(&scene, layer, id, 0);
        assert!(scene.modified_object_at(layer, &obj, 0).is_none());
    }

    #[test]
    fn a_wiggle_modifier_offsets_by_the_noise_value() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Sign", LayerKind::Normal);
        let mut object = Object::shape(
            ObjectId(9),
            ShapeData::filled(Rect::new(-5.0, -5.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
        );
        object.modifiers.push(Modifier::Wiggle {
            amplitude: 10.0,
            frequency: 2.0,
        });
        let id = scene.add_object(layer, object).unwrap();
        // Hold the object across a range so frame 5 resolves.
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(30);
        });

        let obj = resolved(&scene, layer, id, 5);
        let eval = scene.modified_object_at(layer, &obj, 5).expect("a wiggle");
        assert!(eval.object.is_none(), "a wiggle should not re-pose, only offset");

        let fps = scene.stage().frame_rate.max(1.0);
        let want = buzz_physics::wiggle_at(
            buzz_physics::Wiggle {
                amplitude: 10.0,
                frequency: 2.0,
            },
            id.0,
            5.0 / fps,
        );
        let t = eval.prepend.translation();
        assert!((t.x - want.dx).abs() < 1e-9 && (t.y - want.dy).abs() < 1e-9);
    }

    #[test]
    fn a_live_spring_matches_the_solver() {
        let (mut scene, layer, id) = rig_scene();
        animate_base(&mut scene, layer, id, 47);
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Spring {
                root: 1,
                stiffness: 80.0,
                damping: 6.0,
                coupling: 0.0,
            });
        });

        // Reconstruct the primary the same way the evaluator does, and run the
        // solver directly: the live pose must equal it frame for frame.
        let topology = {
            let obj = resolved(&scene, layer, id, 0);
            match obj.kind {
                ObjectKind::Armature(d) => d.armature,
                _ => unreachable!(),
            }
        };
        let span = scene.layers().get(layer).unwrap().frames.length();
        let primary: Vec<Vec<f64>> = (0..span)
            .map(|g| match resolved(&scene, layer, id, g).kind {
                ObjectKind::Armature(d) => d.armature.pose(),
                _ => unreachable!(),
            })
            .collect();
        let fps = scene.stage().frame_rate.max(1.0);
        let expected = buzz_rig::follow_through(
            &topology,
            1,
            buzz_physics::Spring {
                stiffness: 80.0,
                damping: 6.0,
            },
            &primary,
            fps,
        );

        for frame in [4u32, 8, 20, 40] {
            let obj = resolved(&scene, layer, id, frame);
            let eval = scene.modified_object_at(layer, &obj, frame).unwrap();
            let live = match eval.object.unwrap().kind {
                ObjectKind::Armature(d) => d.armature.pose(),
                _ => unreachable!(),
            };
            assert_eq!(live, expected[frame as usize], "frame {frame} differs from the solver");
        }
    }

    /// A square standing on the ground, for the three modifiers that measure
    /// themselves against the drawing's own feet.
    fn standing_square(scene: &mut Scene, id: u64) -> (LayerId, ObjectId) {
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let object = Object::shape(
            ObjectId(id),
            // Top at y = 0, base at y = 100, a hundred wide.
            ShapeData::filled(Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9), Color::WHITE),
        );
        let placed = scene.add_object(layer, object).unwrap();
        scene.update_layer(layer, |l| {
            if l.frames.length() <= 60 {
                l.frames.insert_frame(60);
            }
        });
        (layer, placed)
    }

    /// **Breathing moves the chest and leaves the feet alone.**
    ///
    /// Both halves matter. A breath that lifted the whole drawing would be a
    /// character bobbing off the floor, which is worse than not breathing.
    #[test]
    fn breathing_raises_the_chest_and_keeps_the_feet_down() {
        let mut scene = Scene::empty();
        let (layer, id) = standing_square(&mut scene, 11);
        scene.update_object_across(0, 60, id, |o| {
            o.modifiers.push(Modifier::Breathe {
                rate: 14.0,
                depth: 1.0,
            });
        });

        let mut tops = Vec::new();
        for frame in 0..60u32 {
            let obj = resolved(&scene, layer, id, frame);
            let eval = scene.modified_object_at(layer, &obj, frame).unwrap();
            let feet = eval.prepend * buzz_geom::Point::new(50.0, 100.0);
            assert!(
                (feet.y - 100.0).abs() < 1e-9,
                "frame {frame}: the feet moved to {}",
                feet.y
            );
            tops.push((eval.prepend * buzz_geom::Point::new(50.0, 0.0)).y);
        }

        let lo = tops.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = tops.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            hi - lo > 1.0,
            "the chest barely moved over two and a half seconds: {lo}..{hi}"
        );
        // And not by so much that it reads as a balloon.
        assert!(hi - lo < 12.0, "that is not breathing, it is inflating: {lo}..{hi}");
    }

    /// **Sway bends the top and plants the base.** A tree that pivoted about
    /// its middle would lift its roots out of the ground, which is the reason
    /// this is a shear rather than a rotation.
    #[test]
    fn sway_leans_the_top_and_plants_the_base() {
        let mut scene = Scene::empty();
        let (layer, id) = standing_square(&mut scene, 12);
        scene.update_object_across(0, 60, id, |o| {
            o.modifiers.push(Modifier::Sway {
                amount: 0.3,
                rate: 0.5,
            });
        });

        let mut leans = Vec::new();
        for frame in 0..60u32 {
            let obj = resolved(&scene, layer, id, frame);
            let eval = scene.modified_object_at(layer, &obj, frame).unwrap();
            let base = eval.prepend * buzz_geom::Point::new(50.0, 100.0);
            assert!(
                (base - buzz_geom::Point::new(50.0, 100.0)).hypot() < 1e-9,
                "frame {frame}: the base moved to {base:?}"
            );
            leans.push((eval.prepend * buzz_geom::Point::new(50.0, 0.0)).x);
        }
        let lo = leans.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = leans.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(hi - lo > 2.0, "the top hardly moved: {lo}..{hi}");
    }

    /// **A drift loops, and its phase says where in the loop it starts.**
    ///
    /// The phase is what makes a *field* of drifting things possible: without
    /// it every cloud on one loop crosses the sky in formation.
    #[test]
    fn a_drift_wraps_and_its_phase_offsets_it() {
        let mut scene = Scene::empty();
        let (layer, id) = standing_square(&mut scene, 13);
        scene.update_object_across(0, 60, id, |o| {
            o.modifiers.push(Modifier::Drift {
                dx: 100.0,
                dy: 0.0,
                span: 200.0,
                phase: 0.0,
            });
        });
        let fps = scene.stage().frame_rate.max(1.0);
        let at = |frame: u32| {
            let obj = resolved(&scene, layer, id, frame);
            scene
                .modified_object_at(layer, &obj, frame)
                .unwrap()
                .prepend
                .translation()
                .x
        };

        // A hundred units a second, wrapping every two hundred: back to the
        // start after exactly two seconds.
        assert!(at(0).abs() < 1e-9);
        let two_seconds = (2.0 * fps) as u32;
        assert!(
            at(two_seconds).abs() < 1e-6,
            "it did not come back: {}",
            at(two_seconds)
        );
        assert!(at(fps as u32) > 90.0, "it barely moved in a second");

        // And a half phase starts it half way along.
        let mut other = Scene::empty();
        let (layer2, id2) = standing_square(&mut other, 14);
        other.update_object_across(0, 60, id2, |o| {
            o.modifiers.push(Modifier::Drift {
                dx: 100.0,
                dy: 0.0,
                span: 200.0,
                phase: 0.5,
            });
        });
        let obj = resolved(&other, layer2, id2, 0);
        let offset = other
            .modified_object_at(layer2, &obj, 0)
            .unwrap()
            .prepend
            .translation()
            .x;
        assert!(
            (offset - 100.0).abs() < 1e-6,
            "a half phase should start it half way along, got {offset}"
        );
    }

    #[test]
    fn look_at_turns_the_object_toward_its_target() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let mut object = Object::shape(
            ObjectId(3),
            ShapeData::filled(Rect::new(-5.0, -5.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
        );
        // At (100,100), facing +x. Target straight below (+y), so it should turn
        // a quarter turn.
        object.transform = Affine::translate((100.0, 100.0));
        object.modifiers.push(Modifier::LookAt { x: 100.0, y: 300.0 });
        let id = scene.add_object(layer, object).unwrap();

        let obj = resolved(&scene, layer, id, 0);
        let eval = scene.modified_object_at(layer, &obj, 0).unwrap();
        let c = eval.prepend.as_coeffs();
        let angle = c[1].atan2(c[0]);
        assert!(
            (angle - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "aimed {angle} rad, not down at the target"
        );
    }

    #[test]
    fn squash_stretch_lengthens_along_the_motion() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let mut object = Object::shape(
            ObjectId(4),
            ShapeData::filled(Rect::new(-5.0, -5.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
        );
        object.transform = Affine::translate((0.0, 100.0));
        object.modifiers.push(Modifier::AutoSquashStretch { amount: 0.02 });
        let id = scene.add_object(layer, object).unwrap();

        // Slide it along +x from 0 to 200 over ten frames.
        scene.update_layer(layer, |l| {
            if l.frames.length() <= 10 {
                l.frames.insert_frame(10);
            }
        });
        scene.ensure_keyframe(layer, 10);
        scene.update_object_at(10, id, |o| {
            o.transform = Affine::translate((200.0, 100.0));
        });
        scene.update_layer(layer, |l| {
            l.frames.set_tween(0, Tween::motion());
        });

        let obj = resolved(&scene, layer, id, 5);
        let c = scene.modified_object_at(layer, &obj, 5).unwrap().prepend.as_coeffs();
        // Motion is +x, so the linear part scales x up and y down, and preserves
        // area (x scale * y scale ~= 1).
        assert!(c[0] > 1.1, "x should stretch, was {}", c[0]);
        assert!(c[3] < 0.95, "y should squash, was {}", c[3]);
        assert!((c[0] * c[3] - 1.0).abs() < 0.02, "should preserve area");
    }

    #[test]
    fn the_spring_cache_recomputes_after_an_edit() {
        let (mut scene, layer, id) = rig_scene();
        animate_base(&mut scene, layer, id, 47);
        scene.update_object_across(0, u32::MAX, id, |o| {
            o.modifiers.push(Modifier::Spring {
                root: 1,
                stiffness: 80.0,
                damping: 6.0,
                coupling: 0.0,
            });
        });

        let pose_before = match scene
            .modified_object_at(layer, &resolved(&scene, layer, id, 8), 8)
            .unwrap()
            .object
            .unwrap()
            .kind
        {
            ObjectKind::Armature(d) => d.armature.pose(),
            _ => unreachable!(),
        };

        // Change the primary motion (a bigger swing). This bumps the revision, so
        // the cached spring sequence must be thrown away and rebuilt.
        scene.update_object_at(12, id, |o| {
            if let ObjectKind::Armature(r) = &mut o.kind {
                r.armature.set_pose(&[1.6, 0.0, 0.0, 0.0]);
            }
        });

        let pose_after = match scene
            .modified_object_at(layer, &resolved(&scene, layer, id, 8), 8)
            .unwrap()
            .object
            .unwrap()
            .kind
        {
            ObjectKind::Armature(d) => d.armature.pose(),
            _ => unreachable!(),
        };

        assert_ne!(pose_before, pose_after, "the cache did not recompute after the edit");
    }
}
