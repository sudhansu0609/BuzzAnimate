//! A person, as a rigged object.
//!
//! # Why the tool builds one at all
//!
//! Everything downstream in this crate animates a *skeleton*: a walk is thigh
//! and shin angles over time, a nod is one number on the neck. None of that can
//! be offered to somebody who has not already drawn and rigged a character, and
//! rigging one by hand is an afternoon of work before the first frame of
//! animation exists. So the figure is built here — blocked out from capsules,
//! rigged, and ready to be posed — and the animator replaces its parts with
//! their own drawings whenever they like, because each part is ordinary artwork
//! bound to a bone.
//!
//! # The skeleton is fixed, and that is the point
//!
//! [`Joint`] names every bone and its index never changes. A walk cycle written
//! against `Joint::ThighL` keeps working on any figure this module builds, and
//! a figure whose artwork has been redrawn is still that skeleton. The
//! alternative — searching a rig for something called "thigh" — fails silently
//! on the first character somebody names in their own language.
//!
//! # Proportions
//!
//! Eight heads tall, which is the drawing convention for an adult and the one
//! that makes a blocked figure read as a person rather than as a diagram. Every
//! measurement here is a fraction of [`FigureSpec::height`], so a child is the
//! same figure with a smaller number and — via [`FigureSpec::head_ratio`] — a
//! bigger head, which is most of what makes a drawn child a child.

use std::sync::Arc;

use buzz_geom::{Affine, Point, Rect, Shape as _, Vec2};
use kurbo::RoundedRect;
use buzz_rig::{Armature, Bone};
use buzz_scene::{ArmatureData, Object, ObjectId, ObjectKind, ShapeData};
use peniko::Color;

/// Every bone in the figure, in the order they are pushed.
///
/// The discriminant **is** the bone index — [`Joint::index`] relies on it — so
/// the order here is part of the contract with [`crate::perform`] and may not
/// be shuffled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum Joint {
    /// The root. Runs from the hips up to the base of the ribs, so the whole
    /// figure turns about the hips — which is where a walk's weight shift and a
    /// lean both come from.
    Hips = 0,
    /// Ribs to shoulders. A counter-rotation here against the hips is most of
    /// what makes a walk read as a walk rather than as a puppet sliding along.
    Chest = 1,
    /// Shoulders to the top of the head, as one bone: a neck and a skull that
    /// turn independently is a refinement, and a head that cannot nod is a
    /// missing feature.
    Head = 2,
    ShoulderL = 3,
    ElbowL = 4,
    ShoulderR = 5,
    ElbowR = 6,
    ThighL = 7,
    KneeL = 8,
    ThighR = 9,
    KneeR = 10,
}

impl Joint {
    /// Every joint, in bone order.
    pub const ALL: [Joint; 11] = [
        Joint::Hips,
        Joint::Chest,
        Joint::Head,
        Joint::ShoulderL,
        Joint::ElbowL,
        Joint::ShoulderR,
        Joint::ElbowR,
        Joint::ThighL,
        Joint::KneeL,
        Joint::ThighR,
        Joint::KneeR,
    ];

    /// This joint's index in the armature.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Joint::Hips => "Hips",
            Joint::Chest => "Chest",
            Joint::Head => "Head",
            Joint::ShoulderL => "Shoulder L",
            Joint::ElbowL => "Elbow L",
            Joint::ShoulderR => "Shoulder R",
            Joint::ElbowR => "Elbow R",
            Joint::ThighL => "Thigh L",
            Joint::KneeL => "Knee L",
            Joint::ThighR => "Thigh R",
            Joint::KneeR => "Knee R",
        }
    }

    /// Is this one of the near-side limbs?
    ///
    /// The two sides of a walk are the same curve half a cycle apart, so a
    /// performance asks this rather than listing every joint twice.
    pub fn is_left(self) -> bool {
        matches!(
            self,
            Joint::ShoulderL | Joint::ElbowL | Joint::ThighL | Joint::KneeL
        )
    }
}

/// How the figure is coloured.
///
/// Deliberately flat: what is being built is a blocked figure to animate, and
/// shading it here would be shading the animator has to undo before putting
/// their own drawing in. The document's lights do the shading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub skin: Color,
    pub shirt: Color,
    pub trousers: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            skin: Color::from_rgb8(0xE8, 0xB6, 0x92),
            shirt: Color::from_rgb8(0x3E, 0x6B, 0xA8),
            trousers: Color::from_rgb8(0x33, 0x39, 0x4A),
        }
    }
}

/// What sort of person to build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FigureSpec {
    /// Standing height in document units, from the soles to the top of the
    /// head.
    pub height: f64,
    /// Head height as a fraction of the whole. An eighth is the adult drawing
    /// convention; a sixth reads as a teenager and a quarter as a small child.
    pub head_ratio: f64,
    /// Which way it faces. Positive faces right, negative faces left. Applied
    /// as a mirror on the object's transform, so it costs nothing and does not
    /// need a second skeleton.
    pub facing: f64,
    pub palette: Palette,
}

impl Default for FigureSpec {
    fn default() -> Self {
        Self {
            height: 320.0,
            // Seven rather than the eight a life-drawing class measures. Eight
            // is what a photographed adult is; seven is what animation draws,
            // because a head that reads at the size a shot puts it on screen
            // matters more than a head that is correct in a proportion chart.
            head_ratio: 1.0 / 7.0,
            facing: 1.0,
            palette: Palette::default(),
        }
    }
}

impl FigureSpec {
    /// A child: shorter, and with the larger head that is most of what says so.
    pub fn child() -> Self {
        Self {
            height: 210.0,
            head_ratio: 1.0 / 5.5,
            ..Self::default()
        }
    }

    pub(crate) fn head(&self) -> f64 {
        self.height * self.head_ratio.clamp(0.06, 0.3)
    }
}

/// Straight up, in the angle convention `buzz-rig` uses: the stage measures y
/// downwards, so this is a quarter turn anticlockwise from "along +x".
const UP: f64 = -std::f64::consts::FRAC_PI_2;

/// **Build a figure, rigged and standing at the origin.**
///
/// The object's own coordinates put `(0, 0)` **between the feet**, so placing a
/// figure is one `Affine::translate` to the spot on the ground it stands on —
/// which is the thing an animator is actually choosing, and which keeps a walk
/// cycle's travel honest when the figure is scaled.
///
/// `id` names the wrapper; the parts inside take ids from `next_id`, which the
/// caller wires to the scene's allocator so that nothing collides.
pub fn build(spec: &FigureSpec, id: ObjectId, mut next_id: impl FnMut() -> ObjectId) -> Object {
    let head = spec.head();
    // Eight heads: the usual landmarks, measured from the ground upwards, so
    // every one of them is negative.
    let hips_y = -spec.height * 0.47;
    let chest_y = -spec.height * 0.70;
    // The shoulders sit just under the chin, not a hand's width below it: the
    // head bone runs from here to the crown and only its top is skull, so every
    // unit between the two is neck.
    let shoulder_y = -spec.height * 0.82;
    let crown_y = -spec.height;

    let mut armature = Armature::new(Point::new(0.0, hips_y));

    // The spine, upwards. Lengths and angles rather than end points, because a
    // bone *is* a length and an angle: building it that way keeps the rest pose
    // exactly reconstructible from the numbers stored in the file.
    let hips = armature.push(Bone::new(Joint::Hips.label(), None, hips_y - chest_y, UP));
    let chest = armature.push(Bone::new(
        Joint::Chest.label(),
        Some(hips),
        chest_y - shoulder_y,
        0.0,
    ));
    armature.push(Bone::new(
        Joint::Head.label(),
        Some(chest),
        shoulder_y - crown_y,
        0.0,
    ));

    // Arms, hanging from the shoulders. A hair off vertical so the rest pose
    // has arms beside the body rather than fused to it, and so an IK solve has
    // a direction to bend in rather than a singularity to sit on.
    let upper_arm = spec.height * 0.155;
    let forearm = spec.height * 0.145;
    // **Far enough off vertical to clear the ribs.** An arm that hangs straight
    // down on a figure this wide is inside the torso's own silhouette, so the
    // character reads as armless until it gestures.
    for (shoulder_joint, elbow_joint, lean) in [
        (Joint::ShoulderL, Joint::ElbowL, 0.26),
        (Joint::ShoulderR, Joint::ElbowR, -0.26),
    ] {
        // Measured from the chest bone, which points up: an arm hangs down, so
        // it is half a turn from its parent.
        let shoulder = armature.push(Bone::new(
            shoulder_joint.label(),
            Some(chest),
            upper_arm,
            std::f64::consts::PI + lean,
        ));
        armature.push(Bone::new(elbow_joint.label(), Some(shoulder), forearm, 0.06));
        debug_assert_eq!(shoulder, shoulder_joint.index());
    }

    // **Legs, from the pelvis down — as roots of their own.**
    //
    // A child bone starts at its parent's *tip*, and the hips bone's tip is up
    // at the ribs: hanging the thighs off it put the legs on the chest and left
    // the figure standing on its own sternum. A bone cannot have children at
    // both ends, so the spine goes up from the pelvis and the legs go down from
    // it as separate roots — which is what the armature root is, and where a
    // pair of legs actually starts.
    //
    // The cost is that turning the `Hips` bone leans the torso without carrying
    // the legs with it. At the amplitude a walk uses — three degrees of pelvis
    // sway — that is invisible, and the alternative is a spine that cannot be
    // one bone.
    let thigh = spec.height * 0.235;
    let shin = spec.height * 0.235;
    for (thigh_joint, knee_joint, lean) in [
        (Joint::ThighL, Joint::KneeL, 0.09),
        (Joint::ThighR, Joint::KneeR, -0.09),
    ] {
        let hip = armature.push(Bone::new(
            thigh_joint.label(),
            None,
            thigh,
            std::f64::consts::FRAC_PI_2 + lean,
        ));
        armature.push(Bone::new(knee_joint.label(), Some(hip), shin, -0.04));
        debug_assert_eq!(hip, thigh_joint.index());
    }

    // The pose the artwork is drawn in *is* the rest pose, which is what every
    // later tween is measured against.
    armature.set_rest_here();

    let mut rig = ArmatureData::new(armature);

    // **Artwork, one part per bone, bound rigidly.**
    //
    // Rigid rather than skinned: each limb is its own drawing, so it should
    // turn about its joint rather than deform — which is what `RigBinding`'s
    // own documentation says rigging a chain of symbols means, and what an
    // animator replacing a part with their own drawing will expect.
    let widths = Widths::for_spec(spec);
    for joint in Joint::ALL {
        let Some(part) = limb_artwork(&rig, joint, spec, &widths) else {
            continue;
        };
        rig.bind_rigid(Arc::new(Object::shape(next_id(), part)), joint.index());
    }

    // The head is a circle on the end of the head bone rather than a capsule,
    // because a capsule the width of a skull reads as a thumb.
    let crown = Point::new(0.0, crown_y + head * 0.5);
    rig.bind_rigid(
        Arc::new(Object::shape(
            next_id(),
            ShapeData::filled(
                buzz_geom::Circle::new(crown, head * 0.5).to_path(0.05),
                spec.palette.skin,
            ),
        )),
        Joint::Head.index(),
    );

    Object {
        id,
        // Named, so the Library and the timeline say what it is rather than
        // "Object 47".
        name: Some("Figure".to_string()),
        // Facing is a mirror about the figure's own vertical axis, which passes
        // through the origin because the origin is between the feet.
        transform: Affine::scale_non_uniform(if spec.facing < 0.0 { -1.0 } else { 1.0 }, 1.0),
        kind: ObjectKind::Armature(rig),
        locked: false,
        visible: true,
        filters: Vec::new(),
        blend: buzz_scene::Blend::Normal,
        spatial: Default::default(),
        pivot: None,
        modifiers: Vec::new(),
        text: None,
    }
}

/// How thick each part is drawn.
struct Widths {
    chest: f64,
    hips: f64,
    arm: f64,
    leg: f64,
    neck: f64,
}

impl Widths {
    fn for_spec(spec: &FigureSpec) -> Self {
        Self {
            // **A person is about a quarter as wide as they are tall across the
            // shoulders.** The first version of this used a tenth, which is a
            // drawing of a broom handle: with the arms hanging inside that
            // silhouette the figure had no arms at all until it moved them.
            chest: spec.height * 0.17,
            hips: spec.height * 0.135,
            arm: spec.height * 0.045,
            leg: spec.height * 0.075,
            neck: spec.height * 0.055,
        }
    }
}

/// One limb, drawn as a capsule along the bone it is bound to.
///
/// Built in the **armature's** coordinates from the bone's rest position, which
/// is what a rigid binding expects: the part is drawn where the bone rests, and
/// carried from there by the bone's transform.
///
/// `None` for the head, which is drawn separately.
fn limb_artwork(
    rig: &ArmatureData,
    joint: Joint,
    spec: &FigureSpec,
    widths: &Widths,
) -> Option<ShapeData> {
    let index = joint.index();
    let head = rig.armature.head(index);
    let tip = rig.armature.tip(index);

    let (width, colour) = match joint {
        Joint::Hips => (widths.hips, spec.palette.trousers),
        Joint::Chest => (widths.chest, spec.palette.shirt),
        Joint::ShoulderL | Joint::ShoulderR => (widths.arm, spec.palette.shirt),
        Joint::ElbowL | Joint::ElbowR => (widths.arm, spec.palette.skin),
        Joint::ThighL | Joint::ThighR | Joint::KneeL | Joint::KneeR => {
            (widths.leg, spec.palette.trousers)
        }
        // **The neck.** Drawn here rather than with the skull, because the head
        // bone runs from the shoulders to the crown and the skull only occupies
        // the top of it: without this the head floated a neck's length clear of
        // the shoulders, which is the first thing anyone would notice.
        Joint::Head => (widths.neck, spec.palette.skin),
    };

    Some(ShapeData::filled(capsule(head, tip, width), colour))
}

/// A rounded bar from `head` to `tip`, `width` across.
///
/// Built axis-aligned and then turned onto the bone rather than traced as an
/// outline: a rounded rectangle already knows how to be a capsule, and rotating
/// a path is exact where four hand-built arcs are four places to get a sign
/// wrong.
fn capsule(head: Point, tip: Point, width: f64) -> buzz_geom::BezPath {
    let along = tip - head;
    let length = along.hypot();
    let half = width * 0.5;
    if length <= 1e-9 {
        return buzz_geom::Circle::new(head, half).to_path(0.05);
    }

    // The bar runs from the joint to the tip along +x and is then turned onto
    // the bone. Overshooting each end by half a width is what rounds the joint
    // over: without it the parts meet in a notch that opens as the joint bends.
    let bar = RoundedRect::from_rect(Rect::new(-half, -half, length + half, half), half);
    let turn =
        Affine::translate(head.to_vec2()) * Affine::rotate(Vec2::new(along.x, along.y).atan2());
    turn * bar.to_path(0.05)
}

/// The rest pose of a figure, as [`Armature::pose`] returns it — one angle per
/// bone, in bone order.
///
/// A performance starts from this and adds to it, so that "no motion" really is
/// the figure standing as it was drawn.
pub fn rest_pose(figure: &Object) -> Option<Vec<f64>> {
    match &figure.kind {
        ObjectKind::Armature(rig) => Some(rig.armature.at_rest().pose()),
        _ => None,
    }
}

/// Is this object a rig a performance can drive?
///
/// Checked by shape rather than by a marker: an animator who has replaced every
/// part's artwork still has this skeleton, and somebody else's rig with the
/// same bones can be driven perfectly well.
pub fn is_figure(object: &Object) -> bool {
    match &object.kind {
        ObjectKind::Armature(rig) => rig.armature.len() >= Joint::ALL.len(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn a_figure(spec: FigureSpec) -> Object {
        let mut next = 100u64;
        build(&spec, ObjectId(1), || {
            next += 1;
            ObjectId(next)
        })
    }

    /// The contract every performance in this crate is written against.
    #[test]
    fn the_joints_are_where_the_performance_expects_them() {
        let figure = a_figure(FigureSpec::default());
        let ObjectKind::Armature(rig) = &figure.kind else {
            panic!("a figure is an armature");
        };
        assert_eq!(rig.armature.len(), Joint::ALL.len());
        for joint in Joint::ALL {
            assert_eq!(
                rig.armature.bones[joint.index()].name,
                joint.label(),
                "bone {} is not {}",
                joint.index(),
                joint.label()
            );
        }
    }

    /// **It stands on the ground at its own origin.** Placing a figure is then
    /// one translate to the spot it stands on, which is what makes a walk's
    /// travel and a scene's layout arithmetic rather than guesswork.
    #[test]
    fn the_origin_is_between_the_feet() {
        let figure = a_figure(FigureSpec::default());
        let bounds = figure.bounds();
        assert!(
            bounds.y1.abs() < 12.0,
            "the soles should sit on y = 0, got {bounds:?}"
        );
        assert!(
            (bounds.y0 + 320.0).abs() < 24.0,
            "and the crown near the full height, got {bounds:?}"
        );
    }

    /// Every bone carries a drawing, or the figure has invisible limbs.
    #[test]
    fn every_bone_has_artwork_on_it() {
        let figure = a_figure(FigureSpec::default());
        let ObjectKind::Armature(rig) = &figure.kind else {
            panic!("a figure is an armature");
        };
        for joint in Joint::ALL {
            let found = rig.parts.iter().any(|p| {
                matches!(p.binding, buzz_scene::RigBinding::Rigid(b) if b == joint.index())
            });
            assert!(found, "{} has no artwork", joint.label());
        }
    }

    /// A child is not simply a small adult: the head is a larger share of it,
    /// which is most of what a drawn child *is*.
    #[test]
    fn a_child_has_a_bigger_head_for_its_size() {
        let adult = FigureSpec::default();
        let child = FigureSpec::child();
        assert!(child.height < adult.height);
        assert!(
            child.head() / child.height > adult.head() / adult.height,
            "a child's head is a larger share of it"
        );
    }

    /// Facing left is a mirror rather than a second skeleton, so a performance
    /// written once drives both.
    #[test]
    fn facing_left_mirrors_rather_than_rebuilding() {
        let left = a_figure(FigureSpec {
            facing: -1.0,
            ..FigureSpec::default()
        });
        assert!(
            left.transform.as_coeffs()[0] < 0.0,
            "facing left flips x, got {:?}",
            left.transform
        );
        assert!(is_figure(&left));
    }

    /// **The contract stated in `buzz_rig::pattern`, checked here.**
    ///
    /// `RigPattern::biped` is the slot list the Rigging panel drops artwork
    /// into, and a rig assembled from it is driven by the same performances
    /// that drive a figure built here — by bone *index*. The two skeletons
    /// therefore have to be the same skeleton. Nothing makes that true at
    /// compile time, because one is a table and the other is an enum, so it is
    /// true here or it is not true at all: reordering either table without the
    /// other would animate the wrong limb rather than fail to build.
    #[test]
    fn the_biped_pattern_and_the_built_figure_are_the_same_skeleton() {
        let figure = a_figure(FigureSpec::default());
        let ObjectKind::Armature(rig) = &figure.kind else {
            panic!("a figure is an armature");
        };
        let pattern = buzz_rig::RigPattern::biped();

        assert_eq!(
            pattern.slots.len(),
            rig.armature.len(),
            "the pattern and the figure have different numbers of bones"
        );
        for (index, slot) in pattern.slots.iter().enumerate() {
            let bone = &rig.armature.bones[index];
            assert_eq!(slot.name, bone.name, "slot {index} is not bone {index}");
            assert_eq!(
                slot.parent, bone.parent,
                "{} hangs off a different bone in each",
                slot.name
            );
            assert_eq!(
                slot.name,
                Joint::ALL[index].label(),
                "slot {index} is not Joint::{:?}",
                Joint::ALL[index]
            );
        }
    }

    /// The pattern's default lengths are what an *unfilled* slot gets, so they
    /// have to describe the same person the figure does — otherwise a
    /// character rigged with no chest drawing would come out with a chest of
    /// some other proportion entirely.
    #[test]
    fn the_biped_patterns_proportions_agree_with_the_figures() {
        let spec = FigureSpec::default();
        let figure = a_figure(spec);
        let ObjectKind::Armature(rig) = &figure.kind else {
            panic!("a figure is an armature");
        };
        let pattern = buzz_rig::RigPattern::biped();

        for (index, slot) in pattern.slots.iter().enumerate() {
            let expected = slot.rest_len * spec.height;
            let actual = rig.armature.bones[index].length;
            assert!(
                (expected - actual).abs() < spec.height * 0.005,
                "{} is {actual} long in the figure and {expected} in the pattern",
                slot.name
            );
        }
    }
}
