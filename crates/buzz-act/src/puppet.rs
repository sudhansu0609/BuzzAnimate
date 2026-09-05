//! **A character with a face**, built the way a limited-animation puppet is.
//!
//! # Why this exists next to `figure`
//!
//! [`crate::figure::build`] makes a **body**: thirteen bones with a drawing on
//! each, which is exactly what a walk cycle needs and exactly what a *close
//! shot* has nothing to show. It has no eyes, so nothing can blink; no mouth,
//! so nothing can be lip-synced; and nothing above the shoulders that an
//! animator would recognise as a head to animate.
//!
//! That was the gap: every automatic character in the program was a person seen
//! from far enough away that their face did not matter, and the first thing
//! anyone asks a character to do is talk.
//!
//! # The rig is the one an animator would build
//!
//! Not one rig, two, wired together — because that is what a puppet is:
//!
//! * **The body is a bone rig.** An armature, posed by
//!   [`crate::perform`]. Bones are right for limbs: an arm swings about a
//!   shoulder, and a chain of angles is the cheapest honest way to say so.
//! * **The face is a layer-parented rig.** The eyes, the brows and the mouth
//!   are symbol instances on their own layers, and those layers **follow** the
//!   body's layer — Animate's Parent column. Move the body and the face goes
//!   with it, with nothing keyed on the face at all.
//!
//! Bones would be the wrong tool for the second half. A blink is not a joint
//! angle, and a mouth shape is a *drawing swap* — the thing a bone rig cannot
//! do and a symbol instance does for free.
//!
//! # Everything it makes is ordinary
//!
//! Layers, symbols, instances and modifiers. There is no "puppet" object in the
//! document: pull the face layer off its parent and it is a face on a layer,
//! open the Eyes symbol and it is two ellipses. That is deliberate, and it is
//! what lets an animator replace any part of it with their own drawing without
//! first having to undo what this did.

use std::sync::Arc;

use buzz_geom::{Affine, Point, Shape as _};
use buzz_scene::{
    LayerId, LayerKind, Modifier, Object, ObjectId, ObjectKind, Scene, ShapeData, SymbolId,
    SymbolInstance, SymbolKind,
};
use peniko::Color;

use crate::figure::{self, FigureSpec};

/// The head box every face symbol is drawn in.
///
/// The artwork is authored once at this size and each character places it
/// scaled to their own head, so a cast of three shares one pair of eyes in the
/// library rather than carrying three near-identical drawings. A hundred
/// because it makes every offset below readable as a percentage of the head.
const HEAD_UNITS: f64 = 100.0;

/// What sort of character to build, and where to stand it.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetSpec {
    /// What the character is called. Layers, symbols and objects all take it,
    /// so the timeline reads as a cast list rather than "Object 47".
    pub name: String,
    /// The body: height, proportion, facing and palette.
    pub figure: FigureSpec,
    /// Where the **feet** go, in stage coordinates.
    pub at: Point,
    /// Blinks per minute. Twelve is a resting rate; much past twenty reads as
    /// nerves. Zero leaves the eyes alone.
    pub blink_rate: f64,
    /// Breaths per minute on the body. Fourteen at rest. Zero leaves it still.
    pub breathe_rate: f64,
    /// Eye colour, which is most of what tells one character from another at
    /// the size a face is usually seen.
    pub eyes: Color,
    /// How long the character's layers must last, in frames.
    pub frames: u32,
}

impl Default for PuppetSpec {
    fn default() -> Self {
        Self {
            name: "Person".into(),
            figure: FigureSpec::default(),
            at: Point::new(0.0, 0.0),
            blink_rate: 12.0,
            breathe_rate: 14.0,
            eyes: Color::from_rgb8(0x2A, 0x1C, 0x12),
            frames: 48,
        }
    }
}

/// Everything the build made, so a caller can perform it and talk with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Puppet {
    /// The body's layer. Everything else in the rig follows this one.
    pub body_layer: LayerId,
    /// The armature.
    pub body: ObjectId,
    /// The layer the eyes and brows sit on. Follows [`Self::body_layer`].
    pub face_layer: LayerId,
    /// The layer the mouth sits on, which lip sync writes onto. Its own layer
    /// rather than the face's, because lip sync **replaces** what is on the
    /// frames it writes, and it must not take the eyes with it.
    pub mouth_layer: LayerId,
    pub eyes: ObjectId,
    pub brows: ObjectId,
    /// The library entries. Shared across a cast: a second character built into
    /// the same document reuses these rather than adding its own.
    pub eyes_symbol: SymbolId,
    pub brows_symbol: SymbolId,
    pub mouth_symbol: SymbolId,
    /// Where the mouth belongs on the stage, at rest — what lip sync should be
    /// handed as its placement.
    pub mouth_at: Point,
    /// How large the mouth artwork has to be drawn there.
    pub mouth_scale: f64,
    /// The centre of the head on the stage, at rest.
    pub head: Point,
}

/// **Build a character: a bone-rigged body, and a face parented to it.**
///
/// The library symbols are made once per document and found by name after
/// that, so a cast shares one pair of eyes and one mouth.
pub fn build(scene: &mut Scene, spec: &PuppetSpec) -> Puppet {
    let head_size = spec.figure.height * spec.figure.head_ratio.clamp(0.06, 0.3);
    // Where the head sits in the figure's own coordinates: the origin is
    // between the feet, the crown is a full height up, and the skull occupies
    // the top of the head bone. `figure::build` draws the skull at exactly
    // this point, so the face lands on it rather than near it.
    let head_local = Point::new(0.0, -spec.figure.height + head_size * 0.5);
    let head = Point::new(spec.at.x + head_local.x, spec.at.y + head_local.y);
    let scale = head_size / HEAD_UNITS;

    // -- the body -----------------------------------------------------------
    let body_layer = scene.add_stage_layer(format!("{} Body", spec.name), LayerKind::Normal);
    let id = scene.next_object_id();
    let mut person = figure::build(&spec.figure, id, || scene.next_object_id());
    person.name = Some(spec.name.clone());
    person.transform = Affine::translate(spec.at.to_vec2()) * person.transform;
    let body = scene.add_object(body_layer, person).unwrap_or(id);
    if spec.breathe_rate > 0.0 {
        scene.update_object_across(0, u32::MAX, body, |o| {
            o.modifiers.push(Modifier::Breathe {
                rate: spec.breathe_rate,
                depth: 1.0,
            });
        });
    }

    // -- the library ---------------------------------------------------------
    let eyes_symbol = shared_symbol(scene, "Eyes", |scene, symbol| {
        fill_symbol(scene, symbol, eye_artwork(spec.eyes));
    });
    let brows_symbol = shared_symbol(scene, "Brows", |scene, symbol| {
        fill_symbol(scene, symbol, brow_artwork());
    });
    let mouth_symbol = shared_symbol(scene, "Mouth", |scene, symbol| {
        crate::lipsync::fill_placeholder_mouth(scene, symbol);
    });

    // -- the face ------------------------------------------------------------
    //
    // Above the body in the stack, because a face is drawn on top of a head.
    let face_layer = scene.add_stage_layer(format!("{} Face", spec.name), LayerKind::Normal);
    let mouth_layer = scene.add_stage_layer(format!("{} Mouth", spec.name), LayerKind::Normal);

    let place = Affine::translate(head.to_vec2()) * Affine::scale(scale);
    let brows = scene
        .add_instance_at(face_layer, 0, brows_symbol, place)
        .unwrap_or(ObjectId(0));
    let eyes = scene
        .add_instance_at(face_layer, 0, eyes_symbol, place)
        .unwrap_or(ObjectId(0));

    scene.update_object_across(0, u32::MAX, eyes, |o| {
        o.name = Some(format!("{} Eyes", spec.name));
        if spec.blink_rate > 0.0 {
            // **On the eyes, not on the character.** The lid falls on whatever
            // drawing it is given, the way Sway leans whatever tree it is put
            // on -- so blinking the whole person would squash the person.
            o.modifiers.push(Modifier::Blink {
                rate: spec.blink_rate,
                duration: 0.16,
            });
        }
    });
    scene.update_object_across(0, u32::MAX, brows, |o| {
        o.name = Some(format!("{} Brows", spec.name));
    });

    // Long enough to hold whatever is performed on them. Done before the
    // parenting, so the rest pose is recorded against a layer that exists at
    // its full length.
    let last = spec.frames.max(1).saturating_sub(1);
    for layer in [body_layer, face_layer, mouth_layer] {
        scene.update_stage_layer(layer, |l| {
            if l.frames.length() <= last {
                l.frames.insert_frame(last);
            }
        });
    }

    // -- the parenting -------------------------------------------------------
    //
    // The half of the rig that is not bones. The face has no keyframes of its
    // own and never needs any: the body walks, and the face goes along.
    scene.set_follows(face_layer, Some(body_layer), 0);
    scene.set_follows(mouth_layer, Some(body_layer), 0);
    // **To the head bone, not to the whole body.**
    //
    // Most of a walk's motion is in the bones, and a face linked to the body
    // alone arrives in the right place and then holds still while the skull
    // under it nods and leans out from behind it. At a wide framing nobody
    // notices; open on somebody running and the face slides off the head.
    for layer in [face_layer, mouth_layer] {
        scene.update_stage_layer(layer, |l| {
            l.follows_bone = Some(figure::Joint::Head.index());
        });
    }

    Puppet {
        body_layer,
        body,
        face_layer,
        mouth_layer,
        eyes,
        brows,
        eyes_symbol,
        brows_symbol,
        mouth_symbol,
        // A third of a head below its centre, which is where a mouth is.
        mouth_at: Point::new(head.x, head.y + head_size * 0.26),
        mouth_scale: scale,
        head,
    }
}

/// A library symbol of this name, made once and found by name after that.
///
/// This is the whole of symbol reuse: the second character built into a
/// document gets the *same* `SymbolId` as the first, so recolouring the eyes
/// recolours the cast and the file carries one drawing rather than three.
fn shared_symbol(
    scene: &mut Scene,
    name: &str,
    fill: impl FnOnce(&mut Scene, SymbolId),
) -> SymbolId {
    if let Some(found) = scene.library().find_by_name(name) {
        return found.id;
    }
    let symbol = scene.add_symbol(name, SymbolKind::Graphic, Some("Cast"));
    fill(scene, symbol);
    symbol
}

/// Put a set of shapes on frame 0 of a symbol's first layer.
fn fill_symbol(scene: &mut Scene, symbol: SymbolId, shapes: Vec<(String, ShapeData)>) {
    let Some(layer) = scene
        .library()
        .get(symbol)
        .and_then(|s| s.layers.iter().next())
        .map(|l| l.id)
    else {
        return;
    };
    let ids: Vec<ObjectId> = (0..shapes.len()).map(|_| scene.next_object_id()).collect();
    let objects: Vec<Arc<Object>> = shapes
        .into_iter()
        .zip(ids)
        .map(|((name, shape), id)| {
            let mut object = Object::shape(id, shape);
            object.name = Some(name);
            Arc::new(object)
        })
        .collect();
    scene.library_mut().update(symbol, |s| {
        s.layers.update(layer, |l| {
            l.frames.set_objects(0, objects);
        });
    });
}

/// **Two eyes, drawn in a hundred-unit head.**
///
/// White, iris, pupil and a catchlight, in that order. The catchlight is not
/// decoration: a pupil with no light in it reads as a hole, and it is one
/// circle.
fn eye_artwork(iris: Color) -> Vec<(String, ShapeData)> {
    let white = Color::from_rgb8(0xFA, 0xFA, 0xF6);
    let line = Color::from_rgb8(0x33, 0x28, 0x22);
    let pupil = Color::from_rgb8(0x14, 0x0E, 0x0A);

    let mut out = Vec::new();
    // Eyes sit a little above the middle of the head; the lower half is jaw.
    let y = -6.0;
    for (side, x) in [("L", -21.0), ("R", 21.0)] {
        let ball = kurbo::Ellipse::new(Point::new(x, y), (15.0, 12.0), 0.0).to_path(0.05);
        let mut shape = ShapeData::filled(ball, white);
        shape.stroke = Some(buzz_scene::StrokeSpec::new(line, 2.0));
        out.push((format!("Eye {side}"), shape));
        out.push((
            format!("Iris {side}"),
            ShapeData::filled(
                kurbo::Circle::new(Point::new(x + 1.0, y + 1.0), 7.0).to_path(0.05),
                iris,
            ),
        ));
        out.push((
            format!("Pupil {side}"),
            ShapeData::filled(
                kurbo::Circle::new(Point::new(x + 1.0, y + 1.0), 3.4).to_path(0.05),
                pupil,
            ),
        ));
        out.push((
            format!("Light {side}"),
            ShapeData::filled(
                kurbo::Circle::new(Point::new(x - 2.0, y - 2.5), 1.8).to_path(0.05),
                Color::WHITE,
            ),
        ));
    }
    out
}

/// Two brows, angled very slightly down towards the nose.
///
/// Flat brows read as a doll. The angle is small on purpose: a strong one is an
/// expression, and an expression baked into the rig is one the animator then
/// has to fight.
fn brow_artwork() -> Vec<(String, ShapeData)> {
    let hair = Color::from_rgb8(0x3A, 0x2A, 0x1E);
    let mut out = Vec::new();
    for (side, x, tilt) in [("L", -21.0, 0.10), ("R", 21.0, -0.10)] {
        let bar = kurbo::RoundedRect::new(-13.0, -2.5, 13.0, 2.5, 2.5).to_path(0.05);
        let placed = Affine::translate((x, -24.0)) * Affine::rotate(tilt);
        out.push((
            format!("Brow {side}"),
            ShapeData::filled(placed * bar, hair),
        ));
    }
    out
}

/// Put an instance of `symbol` on `layer` at `frame`, named and placed.
///
/// Small, but every caller here wants the same four lines and one of them
/// getting the order of the transform wrong is a face on somebody's knee.
pub fn place(
    scene: &mut Scene,
    layer: LayerId,
    frame: u32,
    symbol: SymbolId,
    at: Point,
    scale: f64,
    name: &str,
) -> Option<ObjectId> {
    let transform = Affine::translate(at.to_vec2()) * Affine::scale(scale);
    let id = scene.add_instance_at(layer, frame, symbol, transform)?;
    scene.update_object_at(frame, id, |o| o.name = Some(name.to_string()));
    Some(id)
}

/// An instance of `symbol`, showing one frame of it and holding there.
///
/// What a mouth shape is, and what a turnaround view is: a graphic symbol in
/// single-frame mode is the drawing-swap half of a puppet rig.
pub fn single_frame_instance(id: ObjectId, symbol: SymbolId, frame: u32, at: Affine) -> Object {
    let mut instance = SymbolInstance::new(symbol);
    instance.first_frame = frame;
    instance.loop_mode = buzz_scene::LoopMode::SingleFrame;
    let mut object = Object::instance_of(id, symbol).with_transform(at);
    object.kind = ObjectKind::Instance(instance);
    object
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built() -> (Scene, Puppet) {
        let mut scene = Scene::default();
        let puppet = build(
            &mut scene,
            &PuppetSpec {
                name: "Ana".into(),
                at: Point::new(400.0, 700.0),
                frames: 60,
                ..PuppetSpec::default()
            },
        );
        (scene, puppet)
    }

    /// **The face follows the body**, which is the whole of the second rig.
    #[test]
    fn the_face_is_parented_to_the_body() {
        let (scene, puppet) = built();
        let face = scene.layers().get(puppet.face_layer).expect("a face layer");
        assert_eq!(face.follows, Some(puppet.body_layer));
        let mouth = scene.layers().get(puppet.mouth_layer).expect("a mouth layer");
        assert_eq!(mouth.follows, Some(puppet.body_layer));
    }

    /// **The eyes blink and the body breathes** -- and not the other way round,
    /// which would squash the character once a second.
    #[test]
    fn the_modifiers_land_on_the_right_halves() {
        let (scene, puppet) = built();
        let (_, eyes) = scene.find_object(puppet.eyes).expect("eyes");
        assert!(
            eyes.modifiers
                .iter()
                .any(|m| matches!(m, Modifier::Blink { .. })),
            "the eyes do not blink"
        );
        let (_, body) = scene.find_object(puppet.body).expect("a body");
        assert!(
            body.modifiers
                .iter()
                .any(|m| matches!(m, Modifier::Breathe { .. })),
            "the body does not breathe"
        );
        assert!(
            !body
                .modifiers
                .iter()
                .any(|m| matches!(m, Modifier::Blink { .. })),
            "the whole character blinks"
        );
    }

    /// **A cast shares one pair of eyes.** The second character finds the
    /// symbols the first one made rather than adding its own, which is what
    /// makes a library a library.
    #[test]
    fn a_second_character_reuses_the_library() {
        let (mut scene, first) = built();
        let before = scene.library().len();
        let second = build(
            &mut scene,
            &PuppetSpec {
                name: "Ben".into(),
                at: Point::new(900.0, 700.0),
                ..PuppetSpec::default()
            },
        );
        assert_eq!(scene.library().len(), before, "the library grew");
        assert_eq!(second.eyes_symbol, first.eyes_symbol);
        assert_eq!(second.mouth_symbol, first.mouth_symbol);
    }

    /// **The face follows the head bone, not just the body.**
    ///
    /// Most of a walk's motion is in the bones. A face linked to the body alone
    /// arrives where the character arrives and then holds still while the skull
    /// under it nods away from it — which nobody sees at a wide framing and
    /// everybody sees the moment the camera comes in on somebody running.
    #[test]
    fn the_face_moves_when_the_head_bone_does() {
        let (mut scene, puppet) = built();
        let face = scene
            .layers()
            .get(puppet.face_layer)
            .expect("a face layer");
        assert_eq!(
            face.follows_bone,
            Some(figure::Joint::Head.index()),
            "the face is not linked to the head bone"
        );

        let still = scene.layers().inherited_transform(puppet.face_layer, 0u32);

        // Turn the head bone, and nothing else.
        scene.update_object_at(0, puppet.body, |object| {
            if let ObjectKind::Armature(rig) = &mut object.kind {
                let mut pose = rig.armature.pose();
                pose[figure::Joint::Head.index()] += 0.35;
                rig.armature.set_pose(&pose);
            }
        });

        let after = scene.layers().inherited_transform(puppet.face_layer, 0u32);
        assert_ne!(
            still.as_coeffs(),
            after.as_coeffs(),
            "the head turned and the face stayed where it was"
        );
    }

    /// **The face lands on the head**, not near it: the skull is drawn at this
    /// exact point, and a face half a head off is the first thing anyone sees.
    #[test]
    fn the_face_sits_on_the_skull() {
        let (scene, puppet) = built();
        let (_, body) = scene.find_object(puppet.body).expect("a body");
        let skull = match &body.kind {
            ObjectKind::Armature(rig) => rig
                .posed()
                .into_iter()
                .map(|p| p.bounds())
                .reduce(|a, b| a.union(b))
                .expect("artwork"),
            _ => panic!("the body is not rigged"),
        };
        let top = body.transform * Point::new(skull.x0, skull.y0);
        // The head centre is inside the top eighth of the figure.
        assert!(
            puppet.head.y > top.y && puppet.head.y < top.y + 0.2 * 320.0,
            "the head is at {:?}, the artwork starts at {:?}",
            puppet.head,
            top
        );
    }
}
