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
}

impl Modifier {
    /// A short name for the status line and menus.
    pub fn label(&self) -> &'static str {
        match self {
            Modifier::Wiggle { .. } => "Wiggle",
            Modifier::Spring { .. } => "Spring",
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
        frame: u32,
    ) -> Option<ModifierEval> {
        if object.modifiers.is_empty() {
            return None;
        }
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
                        frame as f64 / fps,
                    );
                    prepend = Affine::translate((sample.dx, sample.dy)) * prepend;
                }
                Modifier::Spring {
                    root,
                    stiffness,
                    damping,
                    coupling,
                } => {
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
    use buzz_geom::{Point, Rect, Shape as _};
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
