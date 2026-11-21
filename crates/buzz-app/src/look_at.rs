//! **Who a talking character is talking to** (user 2026-11-21: "can the auto
//! talking animation detect where the other characters are standing, so the
//! eye movements do not seem off centre or like talking to someone else? It
//! shall feel the character is talking to the character in front of them").
//!
//! [`listener_direction`] finds the nearest other character on the stage and
//! says which way the speaker's eyes have to turn to meet it — as the eyes
//! move, in the speaker's **head**, so a character flipped to face left, or
//! scaled, or with its head turned, still looks the right way. The talking
//! command (`fl.buzzAnimate.listener()`) rests the eyes there and glances away
//! from it and back, instead of looking about at random.

use buzz_geom::{Affine, Point, Rect, Vec2};
use buzz_scene::{Object, ObjectId, ObjectKind, Scene};

/// Which way `speaker`'s eyes turn to look at the nearest other character on
/// the stage at `frame`: a unit vector in the head's own space, x to the
/// right and y down, as the eyeballs move (its up–down part kept to half, a
/// face looks across far more than up or down). `None` when the speaker is
/// not a character on the main timeline or nobody else is on the stage.
pub fn listener_direction(scene: &Scene, speaker: ObjectId, frame: u32) -> Option<(f64, f64)> {
    listener_direction_to(scene, speaker, None, frame)
}

/// [`listener_direction`], to `listener` when one is named (a talk range's
/// **Talks to**), else to the nearest other character. A listener named but
/// not on the stage at `frame`, or the speaker itself, falls back to the
/// nearest.
pub fn listener_direction_to(scene: &Scene, speaker: ObjectId, listener: Option<ObjectId>, frame: u32) -> Option<(f64, f64)> {
    let (layer, _) = scene.find_object(speaker)?;
    if scene.stage_layers().get(layer).is_none() {
        return None;
    }
    let stage = stage_characters(scene, frame);
    let (speaker_object, _) = stage.iter().find(|(o, _)| o.id == speaker)?.clone();
    let (head_space, head_centre) = head_of(scene, &speaker_object, frame);
    // The listener: the nearest other character, measured across the stage
    // (who stands beside them matters, not who is a little higher).
    let named = listener.filter(|id| *id != speaker).and_then(|id| stage.iter().find(|(o, _)| o.id == id));
    let listener = match named {
        Some((object, _)) => eye_line(scene, object),
        None => stage
            .iter()
            .filter(|(o, _)| o.id != speaker)
            .map(|(object, _)| eye_line(scene, object))
            .min_by(|a, b| {
                let d = |p: &Point| (p.x - head_centre.x).abs() * 4.0 + (p.y - head_centre.y).abs();
                d(a).total_cmp(&d(b))
            })?,
    };
    let world = listener - head_centre;
    let local = linear(head_space).inverse() * Point::new(world.x, world.y);
    let local = Vec2::new(local.x, local.y);
    let length = local.hypot();
    if !length.is_finite() || length < 1e-6 {
        return None;
    }
    let (x, y) = (local.x / length, local.y / length);
    Some((x, y.clamp(-0.5, 0.5)))
}

/// Every character on the stage at `frame`: its object as it stands there
/// (tweens resolved), and the layer.
fn stage_characters(scene: &Scene, frame: u32) -> Vec<(Object, buzz_scene::LayerId)> {
    let mut out = Vec::new();
    for l in scene.stage_layers().iter() {
        if !l.shown() || l.audio_guide {
            continue;
        }
        for object in l.frames.resolved_at(frame).iter() {
            if let ObjectKind::Instance(i) = &object.kind
                && crate::scene_life::is_character_symbol(scene, i.symbol, 0)
            {
                out.push((object.clone(), l.id));
            }
        }
    }
    out
}

/// The speaker's head: its whole transform from head space to the stage,
/// and its middle on the stage. A character with no head found stands in
/// for it, its eyes taken a little below its top.
fn head_of(scene: &Scene, character: &Object, frame: u32) -> (Affine, Point) {
    let fallback = (character.transform, eye_line(scene, character));
    let Some(def) = character.instance().and_then(|i| scene.library().get(i.symbol)) else {
        return fallback;
    };
    let local = character.instance().map_or(frame, |i| i.resolve_frame(def.kind, frame, def.length()));
    for l in def.layers.iter() {
        for o in l.frames.resolved_at(local).iter() {
            let Some(i) = o.instance() else { continue };
            let name = scene.library().get(i.symbol).map(|s| s.name.to_lowercase()).unwrap_or_default();
            if l.name.eq_ignore_ascii_case("head") || name.contains("head") || name.contains("face") {
                let space = character.transform * o.transform;
                let centre = transform_rect_centre(character.transform, scene.resolved_bounds(o));
                return (space, centre);
            }
        }
    }
    fallback
}

/// Where a character's eyes are on the stage: across its middle, an eighth
/// of the way down from its top.
fn eye_line(scene: &Scene, character: &Object) -> Point {
    let b = scene.resolved_bounds(character);
    Point::new(b.center().x, b.y0 + b.height() * 0.125)
}

fn transform_rect_centre(t: Affine, r: Rect) -> Point {
    t * r.center()
}

fn linear(t: Affine) -> Affine {
    let [a, b, c, d, _, _] = t.as_coeffs();
    Affine::new([a, b, c, d, 0.0, 0.0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::{LayerKind, ShapeData, SymbolKind};
    use std::sync::Arc;

    /// A character symbol (a "head" layer holding a head, a "body" layer),
    /// placed at `at` with `transform`.
    fn character(scene: &mut Scene, name: &str) -> buzz_scene::SymbolId {
        let head = scene.add_symbol(&format!("{name} head"), SymbolKind::Graphic, None);
        let first = scene.library().get(head).unwrap().layers.iter().next().unwrap().id;
        let face = Arc::new(Object::shape(ObjectId(scene.next_object_id().0), ShapeData::filled(buzz_geom::Shape::to_path(&Rect::new(-20.0, -20.0, 20.0, 20.0), 1e-9), peniko::Color::WHITE)));
        scene.library_mut().update(head, |s| {
            s.layers.update(first, |l| {
                l.frames.set_objects(0, vec![face]);
            });
        });
        let body = scene.add_symbol(name, SymbolKind::Graphic, None);
        let first = scene.library().get(body).unwrap().layers.iter().next().unwrap().id;
        let placed = Object::instance_of(ObjectId(scene.next_object_id().0 + 1), head).with_transform(Affine::translate((0.0, -100.0)));
        let torso = Object::shape(ObjectId(scene.next_object_id().0 + 2), ShapeData::filled(buzz_geom::Shape::to_path(&Rect::new(-30.0, -80.0, 30.0, 80.0), 1e-9), peniko::Color::BLACK));
        scene.library_mut().update(body, |s| {
            s.layers.update(first, |l| {
                l.name = "body".into();
                l.frames.set_objects(0, vec![Arc::new(torso)]);
            });
            let mut head_layer = buzz_scene::Layer::new(buzz_scene::LayerId(first.0 + 10_000), "head", LayerKind::Normal);
            head_layer.frames.set_objects(0, vec![Arc::new(placed)]);
            s.layers.insert(0, head_layer);
        });
        body
    }

    fn place(scene: &mut Scene, symbol: buzz_scene::SymbolId, transform: Affine) -> ObjectId {
        let layer = scene.add_layer("character", LayerKind::Normal);
        let id = scene.next_object_id();
        scene.add_object_at(layer, 0, Object::instance_of(id, symbol).with_transform(transform));
        id
    }

    #[test]
    fn the_eyes_turn_to_the_character_beside_them() {
        let mut scene = Scene::default();
        let raju = character(&mut scene, "Raju");
        let sita = character(&mut scene, "Sita");
        let speaker = place(&mut scene, raju, Affine::translate((200.0, 400.0)));
        place(&mut scene, sita, Affine::translate((700.0, 400.0)));
        let (x, y) = listener_direction(&scene, speaker, 0).expect("someone to talk to");
        assert!(x > 0.95 && y.abs() < 0.2, "to the right, level: {x} {y}");

        // Flipped to face left with the listener still on the right, the
        // eyes turn the other way in the head's own space.
        let flipped = place(&mut scene, raju, Affine::translate((100.0, 400.0)) * Affine::scale_non_uniform(-1.0, 1.0));
        let (x, _) = listener_direction(&scene, flipped, 0).expect("someone to talk to");
        assert!(x < -0.9, "mirrored: {x}");
    }

    #[test]
    fn the_nearest_listener_is_the_one_and_nobody_is_none() {
        let mut scene = Scene::default();
        let raju = character(&mut scene, "Raju");
        let sita = character(&mut scene, "Sita");
        let speaker = place(&mut scene, raju, Affine::translate((500.0, 400.0)));
        assert_eq!(listener_direction(&scene, speaker, 0), None, "alone on the stage");
        place(&mut scene, sita, Affine::translate((1400.0, 400.0)));
        place(&mut scene, sita, Affine::translate((300.0, 400.0)));
        let (x, _) = listener_direction(&scene, speaker, 0).unwrap();
        assert!(x < -0.9, "the nearer one, on the left: {x}");
    }

    /// **Talks to** a named character: the eyes turn to that one, though
    /// another stands nearer; the speaker itself, or one not there, gives the
    /// nearest again.
    #[test]
    fn a_named_listener_is_looked_at_though_another_is_nearer() {
        let mut scene = Scene::default();
        let raju = character(&mut scene, "Raju");
        let sita = character(&mut scene, "Sita");
        let speaker = place(&mut scene, raju, Affine::translate((500.0, 400.0)));
        let far = place(&mut scene, sita, Affine::translate((1400.0, 400.0)));
        place(&mut scene, sita, Affine::translate((300.0, 400.0)));
        let (x, _) = listener_direction_to(&scene, speaker, Some(far), 0).unwrap();
        assert!(x > 0.9, "the one named, on the right: {x}");
        let (x, _) = listener_direction_to(&scene, speaker, Some(speaker), 0).unwrap();
        assert!(x < -0.9, "not at itself: the nearest: {x}");
        let (x, _) = listener_direction_to(&scene, speaker, Some(ObjectId(987_654)), 0).unwrap();
        assert!(x < -0.9, "one not on the stage: the nearest: {x}");
    }
}
